//! The DuChinese window (browse, sign in, import), and what the rest of the
//! app does for imported books: analysing them into the Library, checking
//! for new chapters, rebuilding a book when chapters come (keeping its
//! reading stats, place and read mark under its new id), and the reader's
//! notice of new chapters.

use super::api::Session;
use super::data::{Course, Lesson, Level};
use super::store::{self, Estimate, Record, Records};
use super::worker::{Account, EstimateInputs, Event, Make, Task, Worker};
use crate::icons::{self, ARROW_CLOCKWISE, FOLDER_OPEN, MAGNIFYING_GLASS, WARNING, X};
use crate::reader::Notice;
use crate::{App, BatchMode, palette};
use eframe::egui::{self, RichText};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

/// Imported books are checked for new chapters this often (seconds).
const CHECK_EVERY: f64 = 12.0 * 3600.0;
/// The first check, this long after starting.
const FIRST_CHECK: f64 = 10.0;
/// Opening the window checks books not checked for this long (seconds).
const CHECK_ON_OPEN: u64 = 30 * 60;
/// Most cover thumbnails kept uploaded in GPU memory at once.
const MAX_COVERS: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Sort {
    #[default]
    Fit,
    Level,
    Newest,
    Title,
}

impl Sort {
    const ALL: [Sort; 4] = [Sort::Fit, Sort::Level, Sort::Newest, Sort::Title];
    fn label(self) -> &'static str {
        match self {
            Sort::Fit => "Best fit for you",
            Sort::Level => "Level",
            Sort::Newest => "Newest",
            Sort::Title => "Title",
        }
    }
}

#[derive(Default)]
struct SignIn {
    email: String,
    password: String,
    pasted: String,
    show_paste: bool,
    busy: bool,
    error: Option<String>,
}

/// Cheap signature of the inputs used when estimates were last checked, so
/// normal frames don't re-hash word lists or clone/sort `segmenter_words()`.
struct EstimateCheck {
    merged: Arc<hanstat::anki::Knowledge>,
    user_words_hash: u64,
    skip_hapax: bool,
    fold: bool,
    segmenter: hanstat::segment::SegmenterKind,
    catalogue_gen: u64,
}

#[derive(Clone, PartialEq, Eq)]
struct RowsKey {
    search: String,
    stories: bool,
    courses: bool,
    levels: [bool; 6],
    hide_imported: bool,
    sort: Sort,
    catalogue_gen: u64,
    records_gen: u64,
    estimates_gen: u64,
    history_version: u64,
    history_len: usize,
}

struct FrozenRows {
    rows_gen: u64,
    catalogue_gen: u64,
    ids: Vec<u64>,
    indices: Vec<usize>,
}

pub struct DuState {
    pub open: bool,
    worker: Option<Worker>,
    records: Records,
    records_gen: u64,
    account: Account,
    catalogue: Vec<Course>,
    catalogue_gen: u64,
    catalogue_index: HashMap<u64, usize>,
    catalogue_asked: bool,
    estimates: BTreeMap<u64, Estimate>,
    estimates_gen: u64,
    /// What the estimates were last asked for with: known words, and how
    /// many series.
    estimates_asked: Option<(String, usize)>,
    estimate_check: Option<EstimateCheck>,
    estimating: Option<(usize, usize)>,
    covers: HashMap<u64, Option<egui::TextureHandle>>,
    cover_order: VecDeque<u64>,
    covers_asked: HashSet<u64>,
    /// Imports and updates under way: chapters fetched of how many.
    progress: HashMap<u64, (usize, usize)>,
    stories: bool,
    courses: bool,
    levels: [bool; 6],
    search: String,
    hide_imported: bool,
    sort: Sort,
    sign_in: Option<SignIn>,
    error: Option<String>,
    checking: bool,
    /// When books are next checked on their own (egui time).
    next_check: Option<f64>,
    /// Books to analyse into the Library: path, and new (or else updated).
    library: Vec<(PathBuf, bool)>,
    /// The book to open in the reader once rebuilt (Reload in the reader).
    reopen: Option<u64>,
    /// Sent to Build, not back yet.
    building: HashSet<u64>,
    /// Cached filtered and sorted catalogue indices.
    rows_key: Option<RowsKey>,
    rows_cache: Vec<usize>,
    rows_cache_gen: u64,
    /// The list's order while the pointer is over it, and whether it was over it last frame.
    frozen: Option<FrozenRows>,
    hovering: bool,
    /// Cached `is_file()` results for imported books missing from the Library.
    file_exists: HashMap<PathBuf, bool>,
}

impl Default for DuState {
    fn default() -> Self {
        DuState {
            open: false,
            worker: None,
            records: Records::load(),
            records_gen: 0,
            account: Account::default(),
            catalogue: Vec::new(),
            catalogue_gen: 0,
            catalogue_index: HashMap::new(),
            catalogue_asked: false,
            estimates: store::load_estimates(&store::cache_root()),
            estimates_gen: 0,
            estimates_asked: None,
            estimate_check: None,
            estimating: None,
            covers: HashMap::new(),
            cover_order: VecDeque::new(),
            covers_asked: HashSet::new(),
            progress: HashMap::new(),
            stories: true,
            courses: true,
            levels: [true; 6],
            search: String::new(),
            hide_imported: false,
            sort: Sort::Fit,
            sign_in: None,
            error: None,
            checking: false,
            next_check: None,
            library: Vec::new(),
            reopen: None,
            building: HashSet::new(),
            rows_key: None,
            rows_cache: Vec::new(),
            rows_cache_gen: 0,
            frozen: None,
            hovering: false,
            file_exists: HashMap::new(),
        }
    }
}

impl DuState {
    fn worker(&mut self, ctx: &egui::Context) -> &Worker {
        self.worker.get_or_insert_with(|| Worker::start(ctx))
    }

    fn push(&mut self, ctx: &egui::Context, t: Task) {
        self.worker(ctx).push(t);
    }

    fn clear_covers(&mut self) {
        self.covers.clear();
        self.cover_order.clear();
        self.covers_asked.clear();
    }

    /// The sign-in window is open.
    pub fn signing_in(&self) -> bool {
        self.sign_in.is_some()
    }

    pub fn close_sign_in(&mut self) {
        self.sign_in = None;
    }

    /// What the Library shows under an imported book's title: where it
    /// stands, and how many chapters came lately.
    pub fn library_tag(&self, path: &std::path::Path, now: u64) -> Option<(String, u32)> {
        let r = self.records.by_path(path)?;
        Some((r.progress(), r.new_chapters(now)))
    }
}

impl App {
    /// Whether Anki knowledge is still loading/rebuilding.
    fn du_anki_pending(&self) -> bool {
        let anki = &self.anki;
        anki.rx.is_some()
            || anki.dirty
            || (anki.settings.enabled
                && anki.settings.collection.is_some()
                && anki.knowledge.is_none())
    }

    /// The Library entry of an imported book.
    fn du_entry(&self, r: &Record) -> Option<&hanstat::history::Entry> {
        self.history
            .entries
            .iter()
            .find(|e| store::same_file(&e.path, &r.path))
    }

    /// The Library id of the book open in the reader, if it is `r`'s.
    fn du_in_reader(&self, r: &Record) -> bool {
        match (&self.reader, self.du_entry(r)) {
            (Some(reader), Some(e)) => reader.book_id() == e.id,
            _ => false,
        }
    }

    /// Every frame: events from the worker, checks for new chapters, books
    /// waiting to be analysed or rebuilt.
    pub(crate) fn du_tick(&mut self, ctx: &egui::Context) {
        if !self.du.open && !self.du.covers.is_empty() {
            self.du.clear_covers();
        }
        let now = ctx.input(|i| i.time);
        let events: Vec<Event> = match &self.du.worker {
            Some(w) => w.rx.try_iter().collect(),
            None => Vec::new(),
        };
        for e in events {
            self.du_event(e, ctx);
        }
        // Checks on their own.
        if self.settings.duchinese.auto_update && !self.du.records.books.is_empty() {
            let due = *self.du.next_check.get_or_insert(now + FIRST_CHECK);
            if now >= due && !self.du.checking {
                self.du.next_check = Some(now + CHECK_EVERY);
                let t = hanstat::history::now();
                let books: Vec<Record> = self
                    .du
                    .records
                    .books
                    .iter()
                    .filter(|r| {
                        r.pending.is_none()
                            && r.check_due(t)
                            && t.saturating_sub(r.checked) >= (CHECK_EVERY as u64) - 600
                    })
                    .cloned()
                    .collect();
                if !books.is_empty() {
                    self.du.checking = true;
                    self.du.push(
                        ctx,
                        Task::Check {
                            books,
                            revisions: false,
                        },
                    );
                }
            } else if !self.du.checking {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                    (due - now).clamp(1.0, 3600.0),
                ));
            }
        }
        // Updates that waited for the reader.
        let ready: Vec<(Record, Vec<Lesson>)> = self
            .du
            .records
            .books
            .iter()
            .filter(|r| r.pending.is_some() && !self.du.building.contains(&r.course.id))
            .filter(|r| !self.du_in_reader(r))
            .map(|r| (r.clone(), r.pending.clone().unwrap_or_default()))
            .collect();
        for (r, lessons) in ready {
            self.du.building.insert(r.course.id);
            self.du.push(ctx, Task::Build { before: r, lessons });
        }
        // Books to analyse, once nothing else is being analysed.
        if !self.du.library.is_empty()
            && self
                .batch
                .as_ref()
                .is_none_or(|b| b.mode == BatchMode::Auto)
        {
            let new = self.du.library[0].1;
            let (take, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.du.library)
                .into_iter()
                .partition(|x| x.1 == new);
            self.du.library = keep;
            let mut seen = HashSet::new();
            let todo: Vec<_> = take
                .into_iter()
                .filter(|(p, _)| seen.insert(p.clone()))
                .map(|(p, _)| {
                    let label = crate::file_name(&p);
                    match self
                        .history
                        .entries
                        .iter()
                        .find(|e| store::same_file(&e.path, &p))
                    {
                        Some(e) if !new => App::library_item(e),
                        _ => self.batch_item(label, p, self.segmenter),
                    }
                })
                .collect();
            let mode = if new {
                BatchMode::Open
            } else {
                BatchMode::Refresh
            };
            self.start_batch(todo, None, mode, ctx);
        }
    }

    /// The DuChinese windows, when open (not over the reader).
    pub(crate) fn du_windows(&mut self, ctx: &egui::Context) {
        if self.du.open {
            self.du_window(ctx);
        }
        if self.du.sign_in.is_some() {
            self.du_sign_in_window(ctx);
        }
    }

    fn du_event(&mut self, e: Event, ctx: &egui::Context) {
        match e {
            Event::Account(a) => {
                if a.signed_in == Some(true) && self.du.account.signed_in != Some(true) {
                    // Locked chapters may open up: look again.
                    self.du.catalogue_asked = false;
                }
                self.du.account = a;
            }
            Event::Catalogue { courses, .. } => {
                self.du.catalogue_index =
                    courses.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
                self.du.catalogue = courses;
                self.du.catalogue_gen = self.du.catalogue_gen.wrapping_add(1);
                self.du.estimates_asked = None;
                self.du.estimate_check = None;
                // What changed in the series of imported books (chapters announced).
                let mut changed = false;
                for c in &self.du.catalogue {
                    if let Some(r) = self.du.records.get_mut(c.id)
                        && (r.course.lesson_count, r.course.placeholder_count)
                            != (c.lesson_count, c.placeholder_count)
                        && r.course.lesson_count <= c.lesson_count
                    {
                        r.course.lesson_count = c.lesson_count;
                        r.course.placeholder_count = c.placeholder_count;
                        changed = true;
                    }
                }
                if changed {
                    self.du.records_gen = self.du.records_gen.wrapping_add(1);
                    self.du_save();
                }
            }
            Event::Cover(id, img) => {
                if !self.du.open {
                    return;
                }
                let handle = img.map(|ci| {
                    ctx.load_texture(
                        format!("duchinese cover {id}"),
                        ci,
                        egui::TextureOptions::LINEAR,
                    )
                });
                if !self.du.covers.contains_key(&id) {
                    while self.du.covers.len() >= MAX_COVERS {
                        if let Some(old) = self.du.cover_order.pop_front() {
                            self.du.covers.remove(&old);
                            self.du.covers_asked.remove(&old);
                        } else {
                            break;
                        }
                    }
                    self.du.cover_order.push_back(id);
                }
                self.du.covers.insert(id, handle);
            }
            Event::Estimate(id, est) => {
                self.du.estimates.insert(id, est);
                self.du.estimates_gen = self.du.estimates_gen.wrapping_add(1);
            }
            Event::Estimating(p) => self.du.estimating = p,
            Event::Progress {
                course,
                done,
                total,
            } => {
                self.du.progress.insert(course, (done, total));
            }
            Event::Fetched {
                before,
                lessons,
                added,
            } => {
                let id = before.course.id;
                self.du.progress.remove(&id);
                let in_reader = self.du_in_reader(&before);
                if let Some(r) = self.du.records.get_mut(id) {
                    r.course = before.course.clone();
                    r.pending = Some(lessons.clone());
                    if added > 0 {
                        r.added = added;
                        r.added_at = hanstat::history::now();
                    }
                }
                self.du.records_gen = self.du.records_gen.wrapping_add(1);
                self.du_save();
                if !in_reader {
                    self.du.building.insert(id);
                    self.du.push(ctx, Task::Build { before, lessons });
                }
            }
            Event::Unchanged { course, checked } => {
                self.du.progress.remove(&course.id);
                if let Some(r) = self.du.records.get_mut(course.id) {
                    r.checked = checked;
                    if course.lesson_count >= r.course.lesson_count {
                        r.course.lesson_count = course.lesson_count;
                        r.course.placeholder_count = course.placeholder_count;
                    }
                }
                self.du.records_gen = self.du.records_gen.wrapping_add(1);
                self.du_save();
            }
            Event::Built { record, id, before } => self.du_built(record, id, before, ctx),
            Event::Checked => self.du.checking = false,
            Event::Failed { course, error } => {
                if let Some(c) = course {
                    self.du.progress.remove(&c);
                    self.du.building.remove(&c);
                    if self.du.reopen == Some(c) {
                        self.du.reopen = None;
                    }
                }
                if let Some(s) = &mut self.du.sign_in {
                    s.busy = false;
                }
                // Shown in the window; a check on its own says nothing until it is opened.
                let title = course
                    .and_then(|c| {
                        self.du
                            .records
                            .get(c)
                            .map(|r| r.course.title.clone())
                            .or_else(|| {
                                self.du
                                    .catalogue
                                    .iter()
                                    .find(|x| x.id == c)
                                    .map(|x| x.title.clone())
                            })
                    })
                    .map(|t| format!("{}: ", t.trim()))
                    .unwrap_or_default();
                self.du.error = Some(format!("{title}{error}"));
            }
            Event::SignIn(r) => {
                match r {
                    Ok(()) => {
                        self.du.sign_in = None;
                        self.du.error = None;
                        // Partial books can now be completed.
                        let books: Vec<Record> = self
                            .du
                            .records
                            .books
                            .iter()
                            .filter(|r| r.locked > 0)
                            .cloned()
                            .collect();
                        if !books.is_empty() {
                            self.du.checking = true;
                            self.du.push(
                                ctx,
                                Task::Check {
                                    books,
                                    revisions: false,
                                },
                            );
                        }
                    }
                    Err(e) => {
                        if let Some(s) = &mut self.du.sign_in {
                            s.busy = false;
                            s.password.clear();
                            s.error = Some(e);
                        } else {
                            self.du.error = Some(e);
                        }
                    }
                }
            }
        }
    }

    fn du_save(&mut self) {
        if let Err(e) = self.du.records.save() {
            self.du.error = Some(e);
        }
    }

    /// A book written: record it, move what hangs on its old id to the new
    /// one, and analyse it into the Library.
    fn du_built(
        &mut self,
        record: Record,
        id: String,
        before: Option<Record>,
        ctx: &egui::Context,
    ) {
        let course = record.course.id;
        self.du.progress.remove(&course);
        self.du.building.remove(&course);
        let title = record.course.title.trim().to_owned();
        let old_id = self.du_entry(&record).map(|e| e.id.clone());
        if let (Some(old), Some(b)) = (&old_id, &before)
            && *old != id
        {
            // Chapter k of the old book is chapter map[k] of the new one.
            let at: HashMap<&str, u32> = record
                .chapters
                .iter()
                .enumerate()
                .map(|(i, c)| (c.lesson.as_str(), i as u32))
                .collect();
            let map: Vec<u32> = b
                .chapters
                .iter()
                .enumerate()
                .map(|(i, c)| at.get(c.lesson.as_str()).copied().unwrap_or(i as u32))
                .collect();
            self.du_rekey(old, &id, &map, ctx);
        }
        let added = before.as_ref().map_or(0, |b| {
            record.chapters.len().saturating_sub(b.chapters.len())
        });
        let mut record = record;
        if added > 0 {
            record.added = added as u32;
            record.added_at = hanstat::history::now();
        }
        let path = record.path.clone();
        self.du.records.put(record);
        self.du.records_gen = self.du.records_gen.wrapping_add(1);
        self.du.file_exists.clear();
        self.du_save();
        let new = before.is_none() || old_id.is_none();
        self.du.library.push((path.clone(), new));
        let msg = match (&before, added) {
            (None, _) => format!("{title} imported: it is being added to the Library"),
            (Some(_), 0) => format!("{title} updated"),
            (Some(_), n) => format!(
                "{title}: {n} new chapter{} added",
                if n == 1 { "" } else { "s" }
            ),
        };
        self.toast = Some((msg, ctx.input(|i| i.time) + 6.0));
        if self.du.reopen == Some(course) {
            self.du.reopen = None;
            if let Some(e) = self
                .history
                .entries
                .iter()
                .find(|e| store::same_file(&e.path, &path))
            {
                let traditional = e.stats.variant == "Traditional";
                match crate::reader::Reader::open(
                    &path,
                    Some(&e.id),
                    traditional,
                    ctx,
                    &mut self.reader_dictionary,
                ) {
                    Ok(r) => self.reader = Some(r),
                    Err(e) => self.set_error("reader", e),
                }
            }
        }
    }

    /// Book `old` is now `new` (rebuilt): its Library row, reading stats,
    /// place, read mark, reading path entries and writing-style rating
    /// follow it. `spine`: where each old chapter now is.
    fn du_rekey(&mut self, old: &str, new: &str, spine: &[u32], ctx: &egui::Context) {
        self.finish_profile_for(old);
        if let Err(e) = self.history.rekey(old, new) {
            self.history_error = Some(e);
        }
        self.covers.remove(old);
        self.home_view.forget(old);
        self.home_view.forget(new);
        crate::chapter_lengths::forget(old);
        crate::book_series::forget(old);
        self.columns.rekey(old, new);
        crate::reading_stats::rekey(old, new, |s| spine.get(s as usize).copied().unwrap_or(s));
        if let Err(e) = crate::reading_stats::save() {
            self.set_error("reading stats", e);
        }
        // The mark keeps the words it took: new chapters add none.
        let (old_s, new_s) = (old.to_owned(), new.to_owned());
        self.edit_known(ctx, move |k| {
            match k.read.iter_mut().find(|b| b.id == old_s) {
                Some(b) => {
                    b.id = new_s;
                    true
                }
                None => false,
            }
        });
        let st = &mut self.plan.settings;
        let mut changed = false;
        for id in st
            .left_out
            .iter_mut()
            .chain(st.manual.values_mut().flatten())
        {
            if id == old {
                *id = new.to_owned();
                changed = true;
            }
        }
        if changed && let Err(e) = st.save() {
            self.history_error = Some(e);
        }
        if let Some(r) = self.style.store.get(old).cloned() {
            self.style.store.insert(new.to_owned(), r);
            let _ = self.style.store.save();
        }
        self.profiles_version += 1;
    }

    /// The notice the reader shows for an imported book, if any.
    pub(crate) fn du_reader_notice(&self, book_id: &str, last_chapter: bool) -> Option<Notice> {
        let e = self.history.entries.iter().find(|e| e.id == book_id)?;
        let r = self.du.records.by_path(&e.path)?;
        if let Some(p) = &r.pending {
            let (added, _) = super::worker::changes(r, p);
            let text = if added > 0 {
                format!(
                    "{added} new chapter{} out",
                    if added == 1 { " is" } else { "s are" }
                )
            } else {
                "DuChinese edited this story".to_owned()
            };
            return Some(Notice {
                text,
                button: Some("Reload"),
                hover: "Close the book, add the chapters and open it again where you are"
                    .to_owned(),
            });
        }
        let more =
            (r.course.announced() as usize).saturating_sub(r.chapters.len() + r.locked as usize);
        if last_chapter && more > 0 {
            return Some(Notice {
                text: format!(
                    "{more} more chapter{} announced",
                    if more == 1 { "" } else { "s" }
                ),
                button: None,
                hover: if self.settings.duchinese.auto_update {
                    "DuChinese usually releases one a week; hanstat adds them as they come out"
                        .to_owned()
                } else {
                    "DuChinese usually releases one a week; check for them in the DuChinese window"
                        .to_owned()
                },
            });
        }
        if r.locked > 0 && last_chapter {
            return Some(Notice {
                text: format!(
                    "{} more chapter{} need a subscription",
                    r.locked,
                    if r.locked == 1 { "" } else { "s" }
                ),
                button: None,
                hover:
                    "Sign in to DuChinese with a subscription in the DuChinese window, then Update"
                        .to_owned(),
            });
        }
        None
    }

    /// The reader's Reload: close the book and open it again rebuilt.
    pub(crate) fn du_reload(&mut self, ctx: &egui::Context) {
        let Some(id) = self.reader.as_ref().map(|r| r.book_id().to_owned()) else {
            return;
        };
        let Some(path) = self
            .history
            .entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.path.clone())
        else {
            return;
        };
        let Some(r) = self.du.records.by_path(&path).cloned() else {
            return;
        };
        // Its stats end and its place is saved, under the old id: moved over
        // once the book is rebuilt.
        if let Some(reader) = self.reader.take() {
            reader.close(ctx);
        }
        self.du.reopen = Some(r.course.id);
        if !self.du.building.contains(&r.course.id) {
            self.du.building.insert(r.course.id);
            let lessons = r.pending.clone().unwrap_or_default();
            self.du.push(ctx, Task::Build { before: r, lessons });
        }
    }

    /// Ask for the catalogue and the account's state (the window opened).
    pub(crate) fn du_open(&mut self, ctx: &egui::Context) {
        self.du.open = true;
        self.du.file_exists.clear();
        if !self.du.catalogue_asked {
            self.du.catalogue_asked = true;
            self.du.push(ctx, Task::Catalogue { force: false });
            if self.du.account.signed_in.is_none() {
                self.du.push(ctx, Task::Verify);
            }
        }
        let t = hanstat::history::now();
        let books: Vec<Record> = self
            .du
            .records
            .books
            .iter()
            .filter(|r| {
                r.pending.is_none()
                    && r.check_due(t)
                    && t.saturating_sub(r.checked) >= CHECK_ON_OPEN
            })
            .cloned()
            .collect();
        if !books.is_empty() && !self.du.checking {
            self.du.checking = true;
            self.du.push(
                ctx,
                Task::Check {
                    books,
                    revisions: false,
                },
            );
        }
    }

    fn du_import(&mut self, c: &Course, ctx: &egui::Context) {
        let s = &self.settings.duchinese;
        let make = Make {
            traditional: s.traditional,
            translations: s.translations,
            folder: s.folder(),
        };
        self.du.progress.insert(c.id, (0, 0));
        self.du.error = None;
        self.du.push(ctx, Task::Import(c.clone(), make));
    }

    fn du_check(&mut self, books: Vec<Record>, ctx: &egui::Context) {
        for b in &books {
            self.du.progress.insert(b.course.id, (0, 0));
        }
        self.du.checking = true;
        self.du.error = None;
        self.du.push(
            ctx,
            Task::Check {
                books,
                revisions: true,
            },
        );
    }

    fn du_read(&mut self, r: &Record, ctx: &egui::Context) {
        let Some(e) = self.du_entry(r) else { return };
        let (path, id) = (e.path.clone(), e.id.clone());
        let traditional = e.stats.variant == "Traditional";
        match crate::reader::Reader::open(
            &path,
            Some(&id),
            traditional,
            ctx,
            &mut self.reader_dictionary,
        ) {
            Ok(r) => self.reader = Some(r),
            Err(e) => self.du.error = Some(e),
        }
    }

    fn du_cover_by_idx(
        &mut self,
        cat_idx: usize,
        ctx: &egui::Context,
    ) -> Option<egui::TextureHandle> {
        let id = self.du.catalogue[cat_idx].id;
        if let Some(t) = self.du.covers.get(&id) {
            return t.clone();
        }
        if self.du.covers_asked.insert(id) {
            let c = self.du.catalogue[cat_idx].clone();
            self.du.worker(ctx).cover(c);
        }
        None
    }

    fn du_rows_stale(&self) -> bool {
        match &self.du.rows_key {
            Some(k) => {
                k.search != self.du.search
                    || k.stories != self.du.stories
                    || k.courses != self.du.courses
                    || k.levels != self.du.levels
                    || k.hide_imported != self.du.hide_imported
                    || k.sort != self.du.sort
                    || k.catalogue_gen != self.du.catalogue_gen
                    || k.records_gen != self.du.records_gen
                    || k.estimates_gen != self.du.estimates_gen
                    || k.history_version != self.profiles_version
                    || k.history_len != self.history.entries.len()
            }
            None => true,
        }
    }

    /// Recompute `self.du.rows_cache` when any filter, sort, or backing data changed.
    fn du_ensure_rows(&mut self) {
        if !self.du_rows_stale() {
            return;
        }
        self.du.rows_cache = self.du_rows();
        self.du.rows_cache_gen = self.du.rows_cache_gen.wrapping_add(1);
        self.du.rows_key = Some(RowsKey {
            search: self.du.search.clone(),
            stories: self.du.stories,
            courses: self.du.courses,
            levels: self.du.levels,
            hide_imported: self.du.hide_imported,
            sort: self.du.sort,
            catalogue_gen: self.du.catalogue_gen,
            records_gen: self.du.records_gen,
            estimates_gen: self.du.estimates_gen,
            history_version: self.profiles_version,
            history_len: self.history.entries.len(),
        });
    }

    /// The series shown, in order.
    fn du_rows(&self) -> Vec<usize> {
        let search_trimmed = self.du.search.trim();
        let q = (!search_trimmed.is_empty()).then(|| hanstat::search::Query::new(&self.du.search));
        let level_on = |l: Level| match l {
            Level::Newbie => self.du.levels[0],
            Level::Elementary => self.du.levels[1],
            Level::Intermediate => self.du.levels[2],
            Level::UpperIntermediate => self.du.levels[3],
            Level::Advanced => self.du.levels[4],
            Level::Master => self.du.levels[5],
            Level::Unknown => true,
        };
        let imported: HashSet<u64> = if self.du.hide_imported {
            self.du.records.books.iter().map(|r| r.course.id).collect()
        } else {
            HashSet::new()
        };
        let mut rows: Vec<usize> = (0..self.du.catalogue.len())
            .filter(|&i| {
                let c = &self.du.catalogue[i];
                (if c.is_story() {
                    self.du.stories
                } else {
                    self.du.courses
                }) && level_on(c.level())
                    && (!self.du.hide_imported || !imported.contains(&c.id))
                    && q.as_ref()
                        .is_none_or(|q| q.matches([c.title.as_str(), c.description.as_str()]))
            })
            .collect();
        let cat = &self.du.catalogue;
        match self.du.sort {
            Sort::Fit => {
                // Precompute full-book fit once per imported record instead of
                // scanning `records` and `history` inside every sort comparison.
                let book_fit: HashMap<u64, f64> = self
                    .du
                    .records
                    .books
                    .iter()
                    .filter_map(|r| {
                        let k = self.du_entry(r)?.stats.known.as_ref()?;
                        Some((r.course.id, k.running_words))
                    })
                    .collect();
                let mut keyed: Vec<(usize, f64)> = rows
                    .into_iter()
                    .map(|i| {
                        let id = cat[i].id;
                        let score = book_fit
                            .get(&id)
                            .copied()
                            .or_else(|| self.du.estimates.get(&id).map(|e| e.percent))
                            .unwrap_or(-1.0);
                        (i, score)
                    })
                    .collect();
                keyed.sort_by(|a, b| b.1.total_cmp(&a.1));
                rows = keyed.into_iter().map(|(i, _)| i).collect();
            }
            Sort::Level => {
                rows.sort_by_cached_key(|&i| (cat[i].level(), cat[i].title.to_lowercase()));
            }
            Sort::Newest => rows.sort_by(|&a, &b| cat[b].release_at.cmp(&cat[a].release_at)),
            Sort::Title => rows.sort_by_cached_key(|&i| cat[i].title.trim().to_lowercase()),
        }
        rows
    }

    fn du_window(&mut self, ctx: &egui::Context) {
        let anki_pending = self.du_anki_pending();
        let have_knowledge = !anki_pending && self.known.merged.is_some();
        if self.known.merged.is_none() {
            self.du.estimate_check = None;
        }
        // Estimates for the series shown first, then the rest.
        if !self.du.catalogue.is_empty()
            && !anki_pending
            && let Some(merged) = self.known.merged.clone()
        {
            let user_words_hash = {
                let mut h = std::hash::DefaultHasher::new();
                self.user_words.hash(&mut h);
                h.finish()
            };
            let skip_hapax = self.settings.skip_hapax;
            let fold = self.anki.settings.match_across_scripts;
            let segmenter = self.segmenter;
            let catalogue_gen = self.du.catalogue_gen;
            let unchanged = self.du.estimate_check.as_ref().is_some_and(|c| {
                Arc::ptr_eq(&c.merged, &merged)
                    && c.user_words_hash == user_words_hash
                    && c.skip_hapax == skip_hapax
                    && c.fold == fold
                    && c.segmenter == segmenter
                    && c.catalogue_gen == catalogue_gen
            });
            if !unchanged && let Some(lang) = self.lang_inputs() {
                let key = lang.joined();
                self.du.estimate_check = Some(EstimateCheck {
                    merged: merged.clone(),
                    user_words_hash,
                    skip_hapax,
                    fold,
                    segmenter,
                    catalogue_gen,
                });
                if self.du.estimates_asked.as_ref() != Some(&(key.clone(), self.du.catalogue.len()))
                {
                    self.du.estimates_asked = Some((key.clone(), self.du.catalogue.len()));
                    let inputs = EstimateInputs {
                        key,
                        knowledge: merged,
                        kind: segmenter,
                        user_words: self.user_words.clone(),
                        known_words: self.known.settings.segmenter_words(),
                        cache: self.cache.clone(),
                        fold_chars: self.fold_chars(),
                    };
                    self.du_ensure_rows();
                    let mut order: Vec<Course> = self
                        .du
                        .rows_cache
                        .iter()
                        .map(|&i| self.du.catalogue[i].clone())
                        .collect();
                    let set: HashSet<u64> = order.iter().map(|c| c.id).collect();
                    order.extend(
                        self.du
                            .catalogue
                            .iter()
                            .filter(|c| !set.contains(&c.id))
                            .cloned(),
                    );
                    self.du.worker(ctx).estimate(order, inputs);
                }
            }
        }
        let mut open = true;
        let mut import: Option<Course> = None;
        let mut check: Option<Vec<Record>> = None;
        let mut read: Option<Record> = None;
        let mut add_to_library: Option<PathBuf> = None;
        let mut sign_out = false;
        let mut refresh = false;
        let mut settings_changed = false;
        let now = hanstat::history::now();
        if self.du.hovering
            && let Some(f) = &mut self.du.frozen
        {
            if f.catalogue_gen != self.du.catalogue_gen {
                f.indices = f
                    .ids
                    .iter()
                    .filter_map(|id| self.du.catalogue_index.get(id).copied())
                    .collect();
                f.catalogue_gen = self.du.catalogue_gen;
            }
        } else {
            self.du_ensure_rows();
            let needs_freeze = self.du.frozen.as_ref().is_none_or(|f| {
                f.rows_gen != self.du.rows_cache_gen || f.catalogue_gen != self.du.catalogue_gen
            });
            if needs_freeze {
                let ids = self
                    .du
                    .rows_cache
                    .iter()
                    .map(|&i| self.du.catalogue[i].id)
                    .collect();
                self.du.frozen = Some(FrozenRows {
                    rows_gen: self.du.rows_cache_gen,
                    catalogue_gen: self.du.catalogue_gen,
                    ids,
                    indices: self.du.rows_cache.clone(),
                });
            }
        }
        let row_count = self.du.frozen.as_ref().map_or(0, |f| f.indices.len());
        egui::Window::new("DuChinese")
            .id(egui::Id::unique("duchinese-window"))
            .open(&mut open)
            .default_size([760.0, 620.0])
            .min_width(560.0)
            .show(ctx, |ui| {
                // Account.
                ui.horizontal_wrapped(|ui| {
                    let a = &self.du.account;
                    match a.signed_in {
                        Some(true) => {
                            let who = if a.email.is_empty() {
                                "Signed in".to_owned()
                            } else {
                                format!("Signed in as {}", a.email)
                            };
                            ui.label(who);
                            match a.premium {
                                Some(true) => {
                                    ui.label(RichText::new("· Premium").color(palette::good()))
                                        .on_hover_text("Locked chapters come with their text");
                                }
                                Some(false) => {
                                    ui.label(RichText::new("· Free account").color(palette::weak()))
                                        .on_hover_text("Locked chapters need a DuChinese subscription");
                                }
                                None => {}
                            }
                            if ui.small_button("Sign out").clicked() {
                                sign_out = true;
                            }
                        }
                        Some(false) => {
                            ui.label(RichText::new("Not signed in: the first chapter of each story is free.").color(palette::weak()));
                            if ui.button("Sign in…").clicked() {
                                self.du.sign_in = Some(SignIn::default());
                            }
                        }
                        None => {
                            crate::palette::spinner(ui);
                            ui.label(RichText::new(if a.loaded { "Checking your DuChinese session…" } else { "Reading your DuChinese session…" }).color(palette::weak()));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let has_books = !self.du.records.books.is_empty();
                        let r = ui
                            .add_enabled(!self.du.checking && has_books, egui::Button::new(icons::rich(ui, format!("{ARROW_CLOCKWISE} Check for new chapters"))))
                            .on_hover_text("Ask DuChinese about every imported story now: new chapters, chapters opened up by a subscription, and edited ones")
                            .on_disabled_hover_text(if self.du.checking { "Checking…" } else { "No story imported yet" });
                        if r.clicked() {
                            check = Some(self.du.records.books.clone());
                        }
                        if ui
                            .small_button(icons::glyph(ui.ctx(), ARROW_CLOCKWISE, false))
                            .on_hover_text("Fetch the list of stories and courses again")
                            .clicked()
                        {
                            refresh = true;
                        }
                    });
                });
                let mut dismiss_error = false;
                if let Some(e) = &self.du.error {
                    ui.horizontal(|ui| {
                        if ui.small_button(icons::glyph(ui.ctx(), X, false)).on_hover_text("Dismiss").clicked() {
                            dismiss_error = true;
                        }
                        ui.label(icons::rich(ui, RichText::new(format!("{WARNING} {e}")).color(palette::warn())));
                    });
                }
                if dismiss_error {
                    self.du.error = None;
                }
                ui.separator();
                // Filters.
                ui.horizontal_wrapped(|ui| {
                    ui.toggle_value(&mut self.du.stories, "Stories")
                        .on_hover_text("Graded stories told over several chapters");
                    ui.toggle_value(&mut self.du.courses, "Courses")
                        .on_hover_text("Series of separate readings: courses and article series");
                    ui.add_space(8.0);
                    for (i, l) in Level::ALL.into_iter().enumerate() {
                        ui.toggle_value(&mut self.du.levels[i], l.label());
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.du.search)
                            .hint_text(icons::rich(ui, format!("{MAGNIFYING_GLASS} Search")))
                            .desired_width(180.0),
                    );
                    ui.checkbox(&mut self.du.hide_imported, "Hide imported");
                    ui.label("Sort:");
                    egui::ComboBox::from_id_salt("du-sort")
                        .selected_text(self.du.sort.label())
                        .show_ui(ui, |ui| {
                            for s in Sort::ALL {
                                ui.selectable_value(&mut self.du.sort, s, s.label());
                            }
                        });
                    if let Some((done, total)) = self.du.estimating {
                        crate::palette::spinner(ui);
                        ui.label(RichText::new(format!("Working out the fit: {} of {total}", done + 1)).color(palette::weak()))
                            .on_hover_text("Each story's first chapter is fetched once and compared with the words you know; later this is instant");
                    }
                });
                ui.add_space(4.0);
                if self.du.catalogue.is_empty() {
                    ui.horizontal(|ui| {
                        crate::palette::spinner(ui);
                        ui.label("Fetching the list of stories…");
                    });
                }
                let row_h = 56.0;
                let list = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(ui.available_height() - 40.0)
                    .show_rows(ui, row_h, row_count, |ui, range| {
                        for i in range {
                            let cat_idx = self.du.frozen.as_ref().unwrap().indices[i];
                            // Asked for only once shown; loaded/decoded off the UI thread.
                            let cover = self.du_cover_by_idx(cat_idx, ctx);
                            let course_id = self.du.catalogue[cat_idx].id;
                            let unindexed_file_exists = if let Some(r) = self.du.records.get(course_id)
                                && self.du_entry(r).is_none()
                            {
                                if let Some(&exists) = self.du.file_exists.get(&r.path) {
                                    exists
                                } else {
                                    let exists = r.path.is_file();
                                    self.du.file_exists.insert(r.path.clone(), exists);
                                    exists
                                }
                            } else {
                                false
                            };
                            let c = &self.du.catalogue[cat_idx];
                            let rec = self.du.records.get(c.id);
                            let entry = rec.and_then(|r| self.du_entry(r));
                            ui.horizontal(|ui| {
                                ui.set_height(row_h - 6.0);
                                let (w, h) = (36.0, 50.0);
                                match &cover {
                                    Some(t) => {
                                        let [tw, th] = t.size().map(|x| x as f32);
                                        let k = (w / tw).min(h / th);
                                        ui.allocate_ui(egui::vec2(w, h), |ui| {
                                            ui.centered_and_justified(|ui| {
                                                ui.add(egui::Image::new(t).fit_to_exact_size(egui::vec2(tw * k, th * k)).corner_radius(2.0));
                                            });
                                        });
                                    }
                                    None => {
                                        ui.allocate_space(egui::vec2(w, h));
                                    }
                                }
                                let right_w = 250.0;
                                ui.allocate_ui(egui::vec2((ui.available_width() - right_w).max(150.0), h), |ui| {
                                    ui.vertical(|ui| {
                                        ui.add(egui::Label::new(RichText::new(c.title.trim()).strong()).truncate())
                                            .on_hover_text(c.description.trim());
                                        let mut sub = format!("{} · ", c.level().label());
                                        sub.push_str(&match (rec, c.placeholder_count) {
                                            (Some(r), _) => r.progress(),
                                            (None, 0) => format!("{} chapters", c.lesson_count),
                                            (None, p) => format!("{} of {} chapters · ongoing", c.lesson_count, c.lesson_count + p),
                                        });
                                        if let Some(r) = rec {
                                            let n = r.new_chapters(now);
                                            if n > 0 {
                                                sub.push_str(&format!(" · {n} new"));
                                            }
                                        }
                                        ui.label(RichText::new(sub).small().color(palette::weak()));
                                    });
                                });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    // Action.
                                    let busy = self.du.progress.get(&c.id).copied();
                                    if let Some((done, total)) = busy {
                                        let f = if total == 0 { 0.0 } else { done as f32 / total as f32 };
                                        let text = if total == 0 { "Starting…".to_owned() } else { format!("Chapter {} of {total}", done + 1) };
                                        ui.add(egui::ProgressBar::new(f).desired_width(110.0).text(text));
                                    } else if let Some(r) = rec {
                                        if r.pending.is_some() {
                                            ui.label(RichText::new("Update waits for the reader").color(palette::weak()))
                                                .on_hover_text("New chapters are fetched: the book is rebuilt once it is closed in the reader");
                                        } else if entry.is_some() {
                                            if ui.button("Read").on_hover_text("Open it in the reader").clicked() {
                                                read = Some(r.clone());
                                            }
                                            if ui.small_button("Update").on_hover_text("Ask DuChinese for new, opened-up or edited chapters now").clicked() {
                                                check = Some(vec![r.clone()]);
                                            }
                                        } else if unindexed_file_exists {
                                            if ui.button("Add to Library").on_hover_text("The book was imported but isn't in the Library").clicked() {
                                                add_to_library = Some(r.path.clone());
                                            }
                                        } else if ui.button("Import again").on_hover_text("The book's file is gone").clicked() {
                                            import = Some(c.clone());
                                        }
                                    } else if ui
                                        .button("Import")
                                        .on_hover_text(format!(
                                            "Fetch its chapters and add it to the Library as a book, in {}",
                                            if self.settings.duchinese.traditional { "Traditional characters" } else { "Simplified characters" }
                                        ))
                                        .clicked()
                                    {
                                        import = Some(c.clone());
                                    }
                                    // Fit.
                                    ui.allocate_ui(egui::vec2(92.0, h), |ui| {
                                        ui.vertical_centered(|ui| {
                                            let full = entry.and_then(|e| e.stats.known.as_ref());
                                            let est = self.du.estimates.get(&c.id);
                                            let (pct, note) = match (full, est) {
                                                (Some(k), _) => (Some(k.running_words), "whole book".to_owned()),
                                                (None, Some(e)) => (Some(e.percent), format!("from chapter {}", e.chapter)),
                                                (None, None) => (None, String::new()),
                                            };
                                            match pct {
                                                Some(p) => {
                                                    let d = hanstat::anki::Difficulty::from_percent(p);
                                                    ui.label(RichText::new(format!("{p:.1}% known")).color(crate::difficulty_color(d)))
                                                        .on_hover_text("Running words you know (names left out), counted as the Library counts them");
                                                    ui.label(RichText::new(note).small().color(palette::weak()));
                                                }
                                                None => {
                                                    ui.label(RichText::new("–").color(palette::weak())).on_hover_text(if have_knowledge {
                                                        "Not worked out yet"
                                                    } else {
                                                        "Open a Chinese book (or set your known words) first: the fit is worked out from the words you know"
                                                    });
                                                }
                                            }
                                        });
                                    });
                                });
                            });
                            ui.separator();
                        }
                    });
                // The order holds while the pointer is over the list, so a
                // click lands on the row it was aimed at.
                self.du.hovering = ui.rect_contains_pointer(list.inner_rect);
                // Footer: how imports are made.
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    let s = &mut self.settings.duchinese;
                    ui.label("Import in");
                    egui::ComboBox::from_id_salt("du-script")
                        .selected_text(if s.traditional { "Traditional" } else { "Simplified" })
                        .show_ui(ui, |ui| {
                            settings_changed |= ui.selectable_value(&mut s.traditional, false, "Simplified").changed();
                            settings_changed |= ui.selectable_value(&mut s.traditional, true, "Traditional").changed();
                        })
                        .response
                        .on_hover_text("DuChinese's own Traditional text: no conversion. Books imported before keep their script");
                    settings_changed |= ui
                        .checkbox(&mut s.translations, "Sentence translations")
                        .on_hover_text("Each paragraph followed by DuChinese's English, blurred until clicked. For new imports")
                        .changed();
                    settings_changed |= ui
                        .checkbox(&mut s.auto_update, "Add new chapters on their own")
                        .on_hover_text("Check imported stories for new chapters twice a day and add them")
                        .changed();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let folder = s.folder();
                        if ui
                            .small_button(icons::glyph(ui.ctx(), FOLDER_OPEN, false))
                            .on_hover_text(format!("Saved to {}. Click to choose another folder for new imports", folder.display()))
                            .clicked()
                            && let Some(p) = rfd::FileDialog::new().set_directory(&folder).pick_folder()
                        {
                            s.folder = Some(p);
                            settings_changed = true;
                        }
                    });
                });
            });
        if !open {
            self.du.open = false;
            self.du.clear_covers();
        }
        if let Some(path) = add_to_library {
            self.du.library.push((path, true));
        }
        if settings_changed && let Err(e) = self.settings.save() {
            self.set_error("settings", e);
        }
        if refresh {
            self.du.push(ctx, Task::Catalogue { force: true });
        }
        if sign_out {
            self.du.push(ctx, Task::SignOut);
        }
        if let Some(c) = import {
            self.du_import(&c, ctx);
        }
        if let Some(b) = check {
            self.du_check(b, ctx);
        }
        if let Some(r) = read {
            self.du_read(&r, ctx);
        }
    }

    fn du_sign_in_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut submit: Option<Task> = None;
        let Some(s) = &mut self.du.sign_in else {
            return;
        };
        egui::Window::new("Sign in to DuChinese")
            .id(egui::Id::unique("duchinese-sign-in"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.add_enabled_ui(!s.busy, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut s.email).hint_text("Email").desired_width(f32::INFINITY));
                    let pw = ui.add(
                        egui::TextEdit::singleline(&mut s.password)
                            .password(true)
                            .hint_text("Password")
                            .desired_width(f32::INFINITY),
                    );
                    let enter = pw.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.label(
                        RichText::new(format!(
                            "Your password goes to duchinese.net only and isn't saved. The session it gives back is kept {}.",
                            crate::secret::where_kept()
                        ))
                        .small()
                        .color(palette::weak()),
                    );
                    ui.horizontal(|ui| {
                        if ui.link("Signed up with Apple or Google?").clicked() {
                            s.show_paste = !s.show_paste;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let ready = !s.email.trim().is_empty() && !s.password.is_empty();
                            if (ui.add_enabled(ready, egui::Button::new("Sign in")).clicked() || (enter && ready)) && !s.busy {
                                s.busy = true;
                                s.error = None;
                                submit = Some(Task::SignIn {
                                    email: s.email.clone(),
                                    password: std::mem::take(&mut s.password),
                                });
                            }
                            if s.busy {
                                crate::palette::spinner(ui);
                            }
                        });
                    });
                    if s.show_paste {
                        ui.separator();
                        ui.label(
                            RichText::new(
                                "Without a password, use the session of a browser signed in to duchinese.net: open its developer tools (F12) › Application (Chrome) or Storage (Firefox) › Cookies › https://duchinese.net, and copy the value of _reader-server_session.",
                            )
                            .small(),
                        );
                        ui.add(egui::TextEdit::singleline(&mut s.pasted).hint_text("_reader-server_session value").desired_width(f32::INFINITY));
                        let session = Session::from_pasted(&s.pasted);
                        if ui.add_enabled(session.is_some(), egui::Button::new("Use this session")).clicked()
                            && let Some(x) = session
                        {
                            s.busy = true;
                            s.error = None;
                            s.pasted.clear();
                            submit = Some(Task::Paste(x));
                        }
                    }
                });
                if let Some(e) = &s.error {
                    ui.colored_label(palette::error(), e);
                }
            });
        if !open {
            self.du.sign_in = None;
        }
        if let Some(t) = submit {
            self.du.push(ctx, t);
        }
    }
}
