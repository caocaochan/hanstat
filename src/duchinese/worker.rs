//! The work that talks to DuChinese or builds books, off the UI thread.
//!
//! Two threads share one [`Client`] (so requests stay spaced out between
//! them): one for what the user waits on (the catalogue, imports, updates,
//! signing in, covers), one for the fit estimates, which can take minutes
//! the first time and give way to anything else.

use super::api::{Client, Error, Session};
use super::data::{Course, Crd, Lesson, Text};
use super::epub;
use super::store::{self, Catalogue, ChapterRec, Chapters, Estimate, Record};
use crate::secret::Slot;
use eframe::egui;
use hanstat::anki::{BookCoverage, Knowledge};
use hanstat::segment::SegmenterKind;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Where the DuChinese session is kept, encrypted.
pub const SESSION: Slot = Slot {
    what: "DuChinese session",
    file: "duchinese.session",
    entropy: b"hanstat duchinese session",
    service: "hanstat: DuChinese",
    account: "duchinese-session",
    purpose: "duchinese-session",
    label: "hanstat: DuChinese session",
};

/// The catalogue is fetched again after this long.
const CATALOGUE_FOR: u64 = 24 * 3600;
/// A chapter list kept for the estimates (their first chapter rarely changes).
const LESSONS_FOR_ESTIMATES: u64 = 7 * 24 * 3600;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Account {
    /// The session was read (or there is none).
    pub loaded: bool,
    pub email: String,
    /// `None`: not known yet.
    pub signed_in: Option<bool>,
    /// Locked chapters come with their text: a subscription.
    pub premium: Option<bool>,
}

/// What the known words are, for the fit estimates.
#[derive(Clone)]
pub struct EstimateInputs {
    /// The known words and settings, as a digest: estimates made with
    /// another are made again.
    pub key: String,
    pub knowledge: Arc<Knowledge>,
    pub kind: SegmenterKind,
    pub user_words: Vec<String>,
    pub known_words: Vec<String>,
    pub cache: Option<hanstat::cache::Cache>,
    pub fold_chars: bool,
}

/// How an import or a rebuild makes the book.
#[derive(Clone, Debug, PartialEq)]
pub struct Make {
    pub traditional: bool,
    pub translations: bool,
    pub audio: bool,
    pub folder: PathBuf,
}

pub enum Task {
    Catalogue {
        force: bool,
    },
    Cover(Course),
    /// Fetch a story's chapters and build it as a new book.
    Import(Course, Make),
    /// Ask DuChinese about these imported books; `revisions`: also take
    /// edited chapters (asked for by hand), not only new ones.
    Check {
        books: Vec<Record>,
        revisions: bool,
    },
    /// Build an imported book again from its chapters (fetched before).
    Build {
        before: Record,
        lessons: Vec<Lesson>,
    },
    /// Download native chapter audio for an already-imported book (without
    /// rebuilding its EPUB).
    DownloadAudio(Record),
    SignIn {
        email: String,
        password: String,
    },
    Paste(Session),
    SignOut,
    Verify,
}

// Events are few: their size doesn't matter.
#[allow(clippy::large_enum_variant)]
pub enum Event {
    Account(Account),
    Catalogue {
        courses: Vec<Course>,
        #[cfg(test)]
        fetched: u64,
    },
    /// A series' cover, decoded and resized to a thumbnail off the UI thread.
    Cover(u64, Option<egui::ColorImage>),
    Estimate(u64, Estimate),
    /// Estimates worked out of those wanted; `None` once done.
    Estimating(Option<(usize, usize)>),
    /// Chapters fetched of a story being imported or updated.
    Progress {
        course: u64,
        done: usize,
        total: usize,
    },
    /// An imported book with something new: rebuild it (now, or once the
    /// reader closes it).
    Fetched {
        before: Record,
        lessons: Vec<Lesson>,
        added: u32,
    },
    /// Nothing new; the series as now seen (chapters announced).
    Unchanged {
        course: Course,
        checked: u64,
    },
    /// A book written: its record, its file's id, and its record before
    /// (`None`: a new import).
    Built {
        record: Record,
        id: String,
        before: Option<Record>,
    },
    /// Native audio downloaded for an imported book's chapters: `(lesson_id, audio_hash)`.
    AudioDownloaded {
        course: u64,
        chapters: Vec<(String, String)>,
    },
    /// A check of every book asked for is over.
    Checked,
    Failed {
        course: Option<u64>,
        error: String,
    },
    SignIn(Result<(), String>),
}

struct Shared {
    client: Client,
    root: PathBuf,
    tx: Sender<Event>,
    ctx: egui::Context,
    account: Mutex<Account>,
}

impl Shared {
    fn send(&self, e: Event) {
        let _ = self.tx.send(e);
        self.ctx.request_repaint();
    }

    fn set_account(&self, f: impl FnOnce(&mut Account)) {
        let a = {
            let mut a = self.account.lock().unwrap();
            let before = a.clone();
            f(&mut a);
            if *a == before {
                return;
            }
            a.clone()
        };
        self.send(Event::Account(a));
    }

    /// What lessons fetched while signed in say about the subscription.
    fn learn_premium(&self, lessons: &[Lesson]) {
        let signed_in = self.account.lock().unwrap().signed_in == Some(true);
        if !signed_in {
            return;
        }
        if lessons.iter().any(|l| !l.free && l.readable()) {
            self.set_account(|a| a.premium = Some(true));
        } else if lessons.iter().any(|l| !l.free && !l.readable()) {
            self.set_account(|a| a.premium = Some(false));
        }
    }

    fn failed(&self, course: Option<u64>, e: Error) {
        if matches!(e, Error::SignedOut) {
            self.set_account(|a| {
                a.signed_in = Some(false);
                a.premium = None;
            });
        }
        self.send(Event::Failed {
            course,
            error: e.to_string(),
        });
    }

    /// A chapter's text: from the cache, or fetched and kept.
    fn crd(&self, l: &Lesson) -> Result<Crd, Error> {
        let url = l
            .crd_url
            .as_deref()
            .ok_or_else(|| Error::Other(format!("Chapter {} is locked", l.id)))?;
        let path = store::crd_path(&self.root, &l.id, l.crd_hash());
        if let Some(b) = store::read_cached(&path)
            && let Ok(c) = Crd::parse(&b)
        {
            return Ok(c);
        }
        let b = self.client.file(url)?;
        let c = Crd::parse(&b).map_err(Error::Other)?;
        let _ = store::write_cached(&path, &b);
        Ok(c)
    }

    fn cached_crd(&self, l: &Lesson) -> Option<Crd> {
        let b = store::read_cached(&store::crd_path(&self.root, &l.id, l.crd_hash()))?;
        Crd::parse(&b).ok()
    }

    /// A chapter's native MP3 audio: kept in the cache if present, or fetched.
    /// Returns its hash (`None` if the lesson has no audio URL).
    fn audio(&self, l: &Lesson) -> Result<Option<String>, Error> {
        let Some(url) = l.audio_url.as_deref().filter(|u| !u.is_empty()) else {
            return Ok(None);
        };
        let hash = l.audio_hash();
        if hash.is_empty() {
            return Ok(None);
        }
        let path = store::audio_path(&self.root, &l.id, hash);
        if path.is_file() {
            return Ok(Some(hash.to_owned()));
        }
        let bytes = self.client.file(url)?;
        store::write_cached(&path, &bytes).map_err(Error::Other)?;
        Ok(Some(hash.to_owned()))
    }

    fn cover(&self, c: &Course) -> Option<Vec<u8>> {
        let path = store::cover_path(&self.root, c.id);
        if let Some(b) = store::read_cached(&path) {
            return Some(b);
        }
        let url = c
            .medium_image_url
            .as_deref()
            .or(c.large_image_url.as_deref())?;
        let b = self.client.file(url).ok()?;
        let _ = store::write_cached(&path, &b);
        Some(b)
    }

    /// The chapter list, fetched now (and kept for the estimates).
    fn lessons(&self, c: &Course) -> Result<Vec<Lesson>, Error> {
        let lessons = self.client.lessons(c)?;
        self.learn_premium(&lessons);
        let _ = Chapters {
            fetched: hanstat::history::now(),
            lessons: lessons.clone(),
        }
        .save(&self.root, c.id);
        Ok(lessons)
    }
}

/// Decode a cover image and scale it down to thumbnail dimensions (`72×100`)
/// off the UI thread so texture uploads stay tiny and fast.
fn decode_cover(bytes: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width() > 72 || img.height() > 100 {
        img.thumbnail(72, 100)
    } else {
        img
    };
    let rgba = img.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        rgba.as_raw(),
    ))
}

/// The estimates still to work out, and the known words they're for.
type EstimateQueue = Arc<(Mutex<Option<(Vec<Course>, EstimateInputs)>>, Condvar)>;

pub struct Worker {
    tasks: Arc<(Mutex<VecDeque<Task>>, Condvar)>,
    covers: Arc<(Mutex<VecDeque<Course>>, Condvar)>,
    estimates: EstimateQueue,
    /// Bumped by each new request for estimates: the running one stops.
    estimate_gen: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    pub rx: Receiver<Event>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.tasks.1.notify_all();
        self.covers.1.notify_all();
        self.estimates.1.notify_all();
    }
}

impl Worker {
    pub fn start(ctx: &egui::Context) -> Worker {
        let (tx, rx) = channel();
        let shared = Arc::new(Shared {
            client: Client::new(Session::default()),
            root: store::cache_root(),
            tx,
            ctx: ctx.clone(),
            account: Mutex::new(Account::default()),
        });
        let tasks: Arc<(Mutex<VecDeque<Task>>, Condvar)> = Arc::default();
        let covers: Arc<(Mutex<VecDeque<Course>>, Condvar)> = Arc::default();
        let estimates: EstimateQueue = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let estimate_gen = Arc::new(AtomicU64::new(0));
        {
            let (shared, tasks, stop) = (shared.clone(), tasks.clone(), stop.clone());
            std::thread::Builder::new()
                .name("hanstat-duchinese".into())
                .spawn(move || {
                    load_session(&shared);
                    loop {
                        let task = {
                            let mut q = tasks.0.lock().unwrap();
                            loop {
                                if stop.load(Ordering::Relaxed) {
                                    return;
                                }
                                // Covers give way to everything else.
                                let next = q
                                    .iter()
                                    .position(|t| !matches!(t, Task::Cover(_)))
                                    .unwrap_or(0);
                                if let Some(t) = q.remove(next) {
                                    break t;
                                }
                                q = tasks.1.wait(q).unwrap();
                            }
                        };
                        run(&shared, task);
                    }
                })
                .expect("cannot start the DuChinese thread");
        }
        {
            let (shared, covers, tasks, stop) =
                (shared.clone(), covers.clone(), tasks.clone(), stop.clone());
            std::thread::Builder::new()
                .name("hanstat-duchinese-covers".into())
                .spawn(move || {
                    loop {
                        let c = {
                            let mut q = covers.0.lock().unwrap();
                            loop {
                                if stop.load(Ordering::Relaxed) {
                                    return;
                                }
                                if let Some(c) = q.pop_front() {
                                    break c;
                                }
                                q = covers.1.wait(q).unwrap();
                            }
                        };
                        let path = store::cover_path(&shared.root, c.id);
                        if let Some(bytes) = store::read_cached(&path) {
                            shared.send(Event::Cover(c.id, decode_cover(&bytes)));
                        } else {
                            tasks.0.lock().unwrap().push_back(Task::Cover(c));
                            tasks.1.notify_all();
                        }
                    }
                })
                .expect("cannot start the DuChinese cover thread");
        }
        {
            let (shared, estimates, stop, generation) = (
                shared,
                estimates.clone(),
                stop.clone(),
                estimate_gen.clone(),
            );
            std::thread::Builder::new()
                .name("hanstat-duchinese-fit".into())
                .spawn(move || {
                    loop {
                        let (job, my_gen) = {
                            let mut q = estimates.0.lock().unwrap();
                            loop {
                                if stop.load(Ordering::Relaxed) {
                                    return;
                                }
                                if let Some(j) = q.take() {
                                    break (j, generation.load(Ordering::Relaxed));
                                }
                                q = estimates.1.wait(q).unwrap();
                            }
                        };
                        run_estimates(&shared, job.0, &job.1, &|| {
                            stop.load(Ordering::Relaxed)
                                || generation.load(Ordering::Relaxed) != my_gen
                        });
                    }
                })
                .expect("cannot start the DuChinese thread");
        }
        Worker {
            tasks,
            covers,
            estimates,
            estimate_gen,
            stop,
            rx,
        }
    }

    pub fn push(&self, t: Task) {
        self.tasks.0.lock().unwrap().push_back(t);
        self.tasks.1.notify_all();
    }

    /// Load a series' cover off the UI thread (cached files decode immediately
    /// without waiting behind network tasks; missing ones are fetched).
    pub fn cover(&self, c: Course) {
        self.covers.0.lock().unwrap().push_back(c);
        self.covers.1.notify_all();
    }

    /// Work out how much of each of `courses` is known, in that order,
    /// replacing any estimates still to do.
    pub fn estimate(&self, courses: Vec<Course>, inputs: EstimateInputs) {
        self.estimate_gen.fetch_add(1, Ordering::Relaxed);
        *self.estimates.0.lock().unwrap() = Some((courses, inputs));
        self.estimates.1.notify_all();
    }
}

fn load_session(s: &Shared) {
    let session = match SESSION.load() {
        Ok(Some(text)) => serde_json::from_str::<Session>(&text).ok(),
        Ok(None) => None,
        Err(e) => {
            s.send(Event::Failed {
                course: None,
                error: e,
            });
            None
        }
    };
    let has = session.as_ref().is_some_and(|x| !x.cookies.is_empty());
    let email = session
        .as_ref()
        .map(|x| x.email.clone())
        .unwrap_or_default();
    if let Some(x) = session {
        s.client.set_session(x);
    }
    s.set_account(|a| {
        a.loaded = true;
        a.email = email;
        a.signed_in = if has { None } else { Some(false) };
    });
}

fn save_session(s: &Shared) -> Result<(), String> {
    let x = s.client.session();
    if x.cookies.is_empty() {
        SESSION.store("")
    } else {
        SESSION.store(&serde_json::to_string(&x).unwrap())
    }
}

fn run(s: &Shared, task: Task) {
    match task {
        Task::Catalogue { force } => catalogue(s, force),
        Task::Cover(c) => {
            let img = s.cover(&c).and_then(|b| decode_cover(&b));
            s.send(Event::Cover(c.id, img));
        }
        Task::Import(c, make) => {
            let id = c.id;
            if let Err(e) = import(s, c, make) {
                s.failed(Some(id), e);
            }
        }
        Task::Check { books, revisions } => {
            for r in books {
                let id = r.course.id;
                if let Err(e) = check(s, r, revisions) {
                    let offline = matches!(e, Error::Offline(_));
                    s.failed(Some(id), e);
                    if offline {
                        break;
                    }
                }
            }
            s.send(Event::Checked);
        }
        Task::Build { before, lessons } => {
            let id = before.course.id;
            let make = Make {
                traditional: before.traditional,
                translations: before.translations,
                audio: before.audio,
                folder: before.path.parent().map(PathBuf::from).unwrap_or_default(),
            };
            match build(s, &before.course, &lessons, &make, Some(&before)) {
                Ok((record, file_id)) => s.send(Event::Built {
                    record,
                    id: file_id,
                    before: Some(before),
                }),
                Err(e) => s.failed(Some(id), e),
            }
        }
        Task::DownloadAudio(record) => {
            let id = record.course.id;
            if let Err(e) = download_audio(s, record) {
                s.failed(Some(id), e);
            }
        }
        Task::SignIn { email, password } => {
            s.client.set_session(Session::default());
            let r = s.client.sign_in(&email, &password);
            drop(password);
            match r {
                Ok(()) => {
                    let saved = save_session(s);
                    s.set_account(|a| {
                        a.email = email.trim().to_owned();
                        a.signed_in = Some(true);
                        a.premium = None;
                    });
                    s.send(Event::SignIn(saved.map_err(|e| {
                        format!("Signed in, but the session can't be kept for next time: {e}")
                    })));
                }
                Err(e) => {
                    s.client.set_session(Session::default());
                    s.send(Event::SignIn(Err(e.to_string())));
                }
            }
        }
        Task::Paste(session) => {
            s.client.set_session(session);
            match s.client.signed_in() {
                Ok(true) => {
                    let saved = save_session(s);
                    s.set_account(|a| {
                        a.email = String::new();
                        a.signed_in = Some(true);
                        a.premium = None;
                    });
                    s.send(Event::SignIn(saved));
                }
                Ok(false) => {
                    s.client.set_session(Session::default());
                    s.send(Event::SignIn(Err(
                        "DuChinese doesn't know that session: copy it again from a browser signed in to duchinese.net"
                            .to_owned(),
                    )));
                }
                Err(e) => {
                    s.client.set_session(Session::default());
                    s.send(Event::SignIn(Err(e.to_string())));
                }
            }
        }
        Task::SignOut => {
            s.client.set_session(Session::default());
            let saved = save_session(s);
            s.set_account(|a| {
                a.email = String::new();
                a.signed_in = Some(false);
                a.premium = None;
            });
            if let Err(e) = saved {
                s.send(Event::Failed {
                    course: None,
                    error: e,
                });
            }
        }
        Task::Verify => match s.client.signed_in() {
            Ok(yes) => s.set_account(|a| {
                a.signed_in = Some(yes);
                if !yes {
                    a.premium = None;
                }
            }),
            Err(e) => s.failed(None, e),
        },
    }
}

fn catalogue(s: &Shared, force: bool) {
    let cached = Catalogue::load(&s.root);
    let now = hanstat::history::now();
    if !cached.courses.is_empty() {
        s.send(Event::Catalogue {
            courses: cached.courses.clone(),
            #[cfg(test)]
            fetched: cached.fetched,
        });
        if !force && now.saturating_sub(cached.fetched) < CATALOGUE_FOR {
            return;
        }
    }
    let mut courses: Vec<Course> = Vec::new();
    for cat in super::api::CATEGORIES {
        match s.client.courses(cat) {
            Ok(c) => courses.extend(c),
            Err(e) => return s.failed(None, e),
        }
    }
    // Series too new for the listings.
    if let Ok(latest) = s.client.latest() {
        for c in latest.into_iter().filter_map(|l| l.course) {
            courses.push(c);
        }
    }
    let mut seen = HashSet::new();
    courses.retain(|c| c.id != 0 && seen.insert(c.id));
    let cat = Catalogue {
        fetched: now,
        courses,
    };
    let _ = cat.save(&s.root);
    s.send(Event::Catalogue {
        courses: cat.courses,
        #[cfg(test)]
        fetched: now,
    });
}

/// Fetch the texts (and, if `audio`, native audio) of `lessons` that are
/// readable, saying how far it got.
fn fetch_chapters(s: &Shared, course: u64, lessons: &[Lesson], audio: bool) -> Result<(), Error> {
    let want: Vec<&Lesson> = lessons.iter().filter(|l| l.readable()).collect();
    for (i, l) in want.iter().enumerate() {
        s.send(Event::Progress {
            course,
            done: i,
            total: want.len(),
        });
        s.crd(l)?;
        if audio {
            s.audio(l)?;
        }
    }
    Ok(())
}

fn import(s: &Shared, c: Course, make: Make) -> Result<(), Error> {
    s.send(Event::Progress {
        course: c.id,
        done: 0,
        total: 0,
    });
    let lessons = s.lessons(&c)?;
    if !lessons.iter().any(Lesson::readable) {
        return Err(Error::Other(format!(
            "No chapter of {} can be read without a subscription",
            c.title
        )));
    }
    fetch_chapters(s, c.id, &lessons, make.audio)?;
    let course = lessons
        .iter()
        .find_map(|l| l.course.clone())
        .filter(|x| x.id == c.id)
        .unwrap_or(c);
    let (record, id) = build(s, &course, &lessons, &make, None)?;
    s.send(Event::Built {
        record,
        id,
        before: None,
    });
    Ok(())
}

fn download_audio(s: &Shared, record: Record) -> Result<(), Error> {
    let course = record.course.id;
    s.send(Event::Progress {
        course,
        done: 0,
        total: 0,
    });
    let lessons = s.lessons(&record.course)?;
    let by_id: BTreeMap<&str, &Lesson> = lessons
        .iter()
        .filter(|l| l.readable())
        .map(|l| (l.id.as_str(), l))
        .collect();
    let targets: Vec<&Lesson> = record
        .chapters
        .iter()
        .filter_map(|c| by_id.get(c.lesson.as_str()).copied())
        .collect();
    if targets.is_empty() {
        return Err(Error::Other(format!(
            "No chapter of {} can be downloaded right now",
            record.course.title
        )));
    }
    let mut downloaded = Vec::new();
    for (i, l) in targets.iter().enumerate() {
        s.send(Event::Progress {
            course,
            done: i,
            total: targets.len(),
        });
        // Keep the `.crd` cached too, so playback has `syllable_times`.
        s.crd(l)?;
        if let Some(hash) = s.audio(l)? {
            downloaded.push((l.id.clone(), hash));
        }
    }
    if downloaded.is_empty() {
        return Err(Error::Other(format!(
            "{} has no audio available on DuChinese",
            record.course.title
        )));
    }
    s.send(Event::AudioDownloaded {
        course,
        chapters: downloaded,
    });
    Ok(())
}

/// What changed for an imported book: chapters to add (released and
/// readable, not in the book), and chapters edited since.
pub fn changes(before: &Record, lessons: &[Lesson]) -> (u32, u32) {
    let had: BTreeMap<&str, &ChapterRec> = before
        .chapters
        .iter()
        .map(|c| (c.lesson.as_str(), c))
        .collect();
    let added = lessons
        .iter()
        .filter(|l| l.readable() && !had.contains_key(l.id.as_str()))
        .count() as u32;
    let revised = lessons
        .iter()
        .filter(|l| {
            l.readable()
                && had
                    .get(l.id.as_str())
                    .is_some_and(|c| c.crd != l.crd_hash())
        })
        .count() as u32;
    (added, revised)
}

fn check(s: &Shared, before: Record, revisions: bool) -> Result<(), Error> {
    let lessons = s.lessons(&before.course)?;
    let course = lessons
        .iter()
        .find_map(|l| l.course.clone())
        .filter(|x| x.id == before.course.id)
        .unwrap_or_else(|| before.course.clone());
    let (added, revised) = changes(&before, &lessons);
    if added == 0 && (revised == 0 || !revisions) {
        if before.audio {
            // Download any missing or re-recorded audio without rebuilding the EPUB.
            let by_id: BTreeMap<&str, &Lesson> = lessons
                .iter()
                .filter(|l| l.readable() && l.has_audio())
                .map(|l| (l.id.as_str(), l))
                .collect();
            let mut need: Vec<&Lesson> = Vec::new();
            for ch in &before.chapters {
                if let Some(&l) = by_id.get(ch.lesson.as_str()) {
                    let hash = l.audio_hash();
                    let changed = revisions && ch.audio != hash;
                    let missing = ch.audio.is_empty()
                        || !store::audio_path(&s.root, &ch.lesson, &ch.audio).is_file();
                    if changed || missing {
                        need.push(l);
                    }
                }
            }
            if !need.is_empty() {
                let mut downloaded = Vec::new();
                for (i, l) in need.iter().enumerate() {
                    s.send(Event::Progress {
                        course: before.course.id,
                        done: i,
                        total: need.len(),
                    });
                    let _ = s.crd(l);
                    if let Some(hash) = s.audio(l)? {
                        downloaded.push((l.id.clone(), hash));
                    }
                }
                if !downloaded.is_empty() {
                    s.send(Event::AudioDownloaded {
                        course: before.course.id,
                        chapters: downloaded,
                    });
                }
            }
        }
        s.send(Event::Unchanged {
            course,
            checked: hanstat::history::now(),
        });
        return Ok(());
    }
    fetch_chapters(s, before.course.id, &lessons, before.audio)?;
    let mut before = before;
    before.course = course;
    s.send(Event::Fetched {
        before,
        lessons,
        added,
    });
    Ok(())
}

/// The table of contents entry of chapter `n` (1-based).
fn label(n: usize, text: &Text, lesson: &Lesson, course: &Course) -> String {
    if let Some(h) = &text.heading {
        format!("{n}. {h}")
    } else if !lesson.title.trim().is_empty() && lesson.title.trim() != course.title.trim() {
        format!("{n}. {}", lesson.title.trim())
    } else {
        format!("第{n}章")
    }
}

/// Write the book from `lessons` (whose texts are in the cache): a new one
/// in `make.folder`, or `before`'s file again. Its record and file id.
fn build(
    s: &Shared,
    course: &Course,
    lessons: &[Lesson],
    make: &Make,
    before: Option<&Record>,
) -> Result<(Record, String), Error> {
    let readable: Vec<&Lesson> = lessons.iter().filter(|l| l.readable()).collect();
    let locked = lessons.iter().filter(|l| !l.readable()).count() as u32;
    let mut texts = Vec::new();
    for l in &readable {
        let crd = match s.cached_crd(l) {
            Some(c) => c,
            None => s.crd(l)?,
        };
        texts.push(super::data::text(&crd, make.traditional));
    }
    // Chapters already in the book stay, even if DuChinese dropped them.
    // (lesson, text, label, crd_hash, audio_hash, release_at)
    let mut chapters: Vec<(String, Text, String, String, String, String)> = Vec::new();
    for (i, (l, t)) in readable.iter().zip(&texts).enumerate() {
        let audio = if make.audio && l.has_audio() {
            let h = l.audio_hash();
            if !h.is_empty() && store::audio_path(&s.root, &l.id, h).is_file() {
                h.to_owned()
            } else {
                String::new()
            }
        } else {
            before
                .and_then(|b| b.chapters.iter().find(|c| c.lesson == l.id))
                .map(|c| c.audio.clone())
                .unwrap_or_default()
        };
        chapters.push((
            l.id.clone(),
            t.clone(),
            label(i + 1, t, l, course),
            l.crd_hash().to_owned(),
            audio,
            l.release_at.clone().unwrap_or_default(),
        ));
    }
    if let Some(b) = before {
        for (k, old) in b.chapters.iter().enumerate() {
            if chapters.iter().any(|c| c.0 == old.lesson) {
                continue;
            }
            let path = store::crd_path(&s.root, &old.lesson, &old.crd);
            let Some(crd) = store::read_cached(&path).and_then(|b| Crd::parse(&b).ok()) else {
                continue;
            };
            let at = k.min(chapters.len());
            chapters.insert(
                at,
                (
                    old.lesson.clone(),
                    super::data::text(&crd, make.traditional),
                    old.label.clone(),
                    old.crd.clone(),
                    old.audio.clone(),
                    old.release.clone(),
                ),
            );
        }
    }
    if chapters.is_empty() {
        return Err(Error::Other(format!(
            "{} has no chapter to read",
            course.title
        )));
    }
    let cover = s.cover(course);
    let newest = chapters
        .iter()
        .map(|c| c.5.as_str())
        .max()
        .unwrap_or("")
        .to_owned();
    let bytes = epub::build(&epub::Book {
        course: course.id,
        title: course.title.trim(),
        description: &course.description,
        source: &course.page(),
        traditional: make.traditional,
        translations: make.translations,
        modified: &newest,
        cover: cover.as_deref(),
        chapters: chapters
            .iter()
            .map(|c| epub::Chapter {
                lesson: &c.0,
                label: c.2.clone(),
                text: &c.1,
            })
            .collect(),
    })
    .map_err(Error::Other)?;
    let path = match before {
        Some(b) => b.path.clone(),
        None => free_path(
            &make.folder,
            &store::file_name(&course.title, make.traditional),
            course.id,
        ),
    };
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)
            .map_err(|e| Error::Other(format!("Can't make {}: {e}", d.display())))?;
    }
    let tmp = path.with_extension("epub.part");
    std::fs::write(&tmp, &bytes)
        .map_err(|e| Error::Other(format!("Can't write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::Other(format!(
            "Can't replace {} (is it open in another program?): {e}",
            path.display()
        ))
    })?;
    let id = hanstat::history::file_id(&path)
        .map_err(|e| Error::Other(format!("Can't read {} back: {e}", path.display())))?;
    let now = hanstat::history::now();
    let record = Record {
        course: course.clone(),
        path,
        traditional: make.traditional,
        translations: make.translations,
        audio: make.audio,
        chapters: chapters
            .into_iter()
            .map(|c| ChapterRec {
                lesson: c.0,
                label: c.2,
                crd: c.3,
                audio: c.4,
                release: c.5,
            })
            .collect(),
        locked,
        checked: now,
        added: before.map_or(0, |b| b.added),
        added_at: before.map_or(0, |b| b.added_at),
        pending: None,
    };
    Ok((record, id))
}

/// `name` in `folder`, or with the series id added if a file of that name
/// is already there (another series of the same title, or the user's own).
fn free_path(folder: &std::path::Path, name: &str, course: u64) -> PathBuf {
    let p = folder.join(name);
    if !p.exists() {
        return p;
    }
    let stem = name.strip_suffix(".epub").unwrap_or(name);
    folder.join(format!("{stem} [{course}].epub"))
}

fn run_estimates(
    s: &Shared,
    courses: Vec<Course>,
    inputs: &EstimateInputs,
    stopped: &dyn Fn() -> bool,
) {
    let mut all = store::load_estimates(&s.root);
    let total = courses.len();
    let mut changed = false;
    let mut sent_estimating = false;
    let mut last_progress: Option<Instant> = None;
    for (i, c) in courses.into_iter().enumerate() {
        if stopped() {
            break;
        }
        if all.get(&c.id).is_some_and(|e| {
            e.key
                .strip_prefix(&inputs.key)
                .is_some_and(|rest| rest.starts_with('|'))
        }) {
            continue;
        }
        if last_progress.is_none_or(|t| t.elapsed() >= Duration::from_millis(50)) {
            s.send(Event::Estimating(Some((i, total))));
            last_progress = Some(Instant::now());
            sent_estimating = true;
        }
        match estimate_one(s, &c, inputs, all.get(&c.id)) {
            Ok(Some(e)) => {
                all.insert(c.id, e.clone());
                changed = true;
                s.send(Event::Estimate(c.id, e));
            }
            Ok(None) => {}
            Err(Error::Offline(_)) => break,
            Err(_) => {}
        }
        if changed && i % 10 == 9 {
            let _ = store::save_estimates(&s.root, &all);
            changed = false;
        }
    }
    if changed {
        let _ = store::save_estimates(&s.root, &all);
    }
    if sent_estimating {
        s.send(Event::Estimating(None));
    }
}

/// The estimate for `c`, unless `have` is still current (`None`).
fn estimate_one(
    s: &Shared,
    c: &Course,
    inputs: &EstimateInputs,
    have: Option<&Estimate>,
) -> Result<Option<Estimate>, Error> {
    let now = hanstat::history::now();
    let lessons = match Chapters::load(&s.root, c.id) {
        Some(ch) if now.saturating_sub(ch.fetched) < LESSONS_FOR_ESTIMATES => ch.lessons,
        _ => s.lessons(c)?,
    };
    let Some((n, first)) = lessons.iter().enumerate().find(|(_, l)| l.readable()) else {
        return Ok(None);
    };
    let key = format!("{}|{}", inputs.key, first.crd_hash());
    if have.is_some_and(|e| e.key == key) {
        return Ok(None);
    }
    let crd = s.crd(first)?;
    let text = super::data::text(&crd, false);
    let percent = known_share(&text, inputs).map_err(Error::Other)?;
    Ok(Some(Estimate {
        key,
        percent,
        chapter: n as u32 + 1,
    }))
}

/// Percent of the running words of `text` known, as the Library counts
/// them (names left out), with every word counted.
pub fn known_share(text: &Text, inputs: &EstimateInputs) -> Result<f64, String> {
    use hanstat::extract::{Book, Section, SectionKind};
    use hanstat::{analyze, discover};
    let mut paragraphs: Vec<String> = text.heading.iter().cloned().collect();
    paragraphs.extend(text.paragraphs.iter().map(|p| p.text.clone()));
    let book = Book {
        title: None,
        format: "EPUB",
        sections: vec![Section {
            label: "1".into(),
            href: None,
            kind: SectionKind::Body,
            paragraphs,
        }],
        notes: Vec::new(),
        cover: None,
    };
    let (tr, si) = analyze::detect_variant(&book);
    if let Some(m) =
        hanstat::segment::model_for(inputs.kind.resolve(hanstat::script::classify(tr, si)))
        && !m.is_cached()
    {
        m.ensure(&|_, _| {})?;
    }
    let prep = analyze::analyze_book_cached(
        &book,
        inputs.kind,
        &inputs.user_words,
        &inputs.known_words,
        inputs.cache.as_ref(),
        &|_, _| {},
    )?;
    let report = analyze::build_report(&book, &prep, &[true], inputs.fold_chars);
    let have: HashSet<String> = inputs.user_words.iter().cloned().collect();
    let mut names: HashSet<String> = have.clone();
    names.extend(
        discover::discover(
            &book,
            &[true],
            &report.word_freq,
            &have,
            &discover::Options::for_size(report.han_total),
        )
        .into_iter()
        .filter(|c| c.recommended)
        .map(|c| c.word),
    );
    let cov = BookCoverage::compute(
        &inputs.knowledge,
        &report.char_freq,
        &report.word_freq,
        &names,
        false,
    );
    Ok(cov.running_words.percent())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lesson(id: &str, hash: Option<&str>) -> Lesson {
        Lesson {
            id: id.into(),
            crd_url: hash.map(|h| format!("https://static.duchinese.net/documents/{id}/{h}.crd")),
            ..Lesson::default()
        }
    }

    #[test]
    fn what_an_update_brings() {
        let before = Record {
            chapters: vec![
                ChapterRec {
                    lesson: "1".into(),
                    crd: "a".into(),
                    ..ChapterRec::default()
                },
                ChapterRec {
                    lesson: "2".into(),
                    crd: "b".into(),
                    ..ChapterRec::default()
                },
            ],
            ..Record::default()
        };
        let now = [
            lesson("1", Some("a")),
            lesson("2", Some("b2")),
            lesson("3", Some("c")),
            lesson("4", None),
        ];
        assert_eq!(changes(&before, &now), (1, 1));
        assert_eq!(changes(&before, &now[..2]), (0, 1));
    }

    /// Against duchinese.net, signed out (free chapters only). Run with
    /// `LOCALAPPDATA` pointed at a scratch folder:
    /// `cargo test --release --bin hanstat duchinese_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn duchinese_live() {
        use std::time::{Duration, Instant};
        let ctx = egui::Context::default();
        let w = Worker::start(&ctx);
        let wait = |what: &str, f: &mut dyn FnMut(Event) -> bool| {
            let until = Instant::now() + Duration::from_secs(300);
            while Instant::now() < until {
                match w.rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(Event::Failed { course, error }) => panic!("{what}: {course:?} {error}"),
                    Ok(e) => {
                        if f(e) {
                            return;
                        }
                    }
                    Err(_) => {}
                }
            }
            panic!("{what}: timed out");
        };
        wait(
            "session",
            &mut |e| matches!(e, Event::Account(a) if a.loaded),
        );

        w.push(Task::Catalogue { force: true });
        let mut courses = Vec::new();
        wait("catalogue", &mut |e| match e {
            Event::Catalogue {
                courses: c,
                fetched,
            } if fetched > 0 => {
                courses = c;
                true
            }
            _ => false,
        });
        println!("{} series", courses.len());
        assert!(courses.len() > 100);
        let peach = courses
            .iter()
            .find(|c| c.id == 38)
            .expect("Oath of the Peach Garden")
            .clone();
        let house = courses
            .iter()
            .find(|c| c.id == 124)
            .expect("The House at Number Thirty-Three")
            .clone();

        let dir = tempfile::tempdir().unwrap();
        let make = Make {
            traditional: false,
            translations: true,
            audio: false,
            folder: dir.path().to_path_buf(),
        };
        w.push(Task::Import(peach.clone(), make.clone()));
        let mut built = None;
        wait("import", &mut |e| match e {
            Event::Built {
                record,
                id,
                before: None,
            } => {
                built = Some((record, id));
                true
            }
            _ => false,
        });
        let (record, id) = built.unwrap();
        println!("{} → {} ({id})", record.course.title, record.path.display());
        assert_eq!(record.chapters.len(), 6);
        assert_eq!(record.locked, 0);
        let labels: Vec<&str> = record.chapters.iter().map(|c| c.label.as_str()).collect();
        println!("{labels:?}");
        let book =
            hanstat::extract::load(&record.path, hanstat::extract::EncodingChoice::Auto).unwrap();
        let han: usize = book
            .sections
            .iter()
            .flat_map(|s| &s.paragraphs)
            .map(|p| {
                p.chars()
                    .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
                    .count()
            })
            .sum();
        println!("{han} Han characters, cover: {}", book.cover.is_some());
        assert!(han > 2000);
        let epub = crate::reader::epub::Epub::open(&record.path).unwrap();
        assert_eq!(epub.spine.len(), 6);

        // As if the last chapter had just come out.
        let mut older = record.clone();
        older.chapters.pop();
        w.push(Task::Check {
            books: vec![older],
            revisions: false,
        });
        let mut fetched = None;
        wait("check", &mut |e| match e {
            Event::Fetched {
                before,
                lessons,
                added,
            } => {
                assert_eq!(added, 1);
                fetched = Some((before, lessons));
                true
            }
            _ => false,
        });
        let (before, lessons) = fetched.unwrap();
        w.push(Task::Build { before, lessons });
        wait("rebuild", &mut |e| match e {
            Event::Built {
                record: r,
                id: again,
                before: Some(_),
            } => {
                assert_eq!(r.chapters.len(), 6);
                assert_eq!(again, id, "the same chapters make the same file");
                true
            }
            _ => false,
        });

        // Signed out, a premium story has one chapter to read.
        w.push(Task::Import(house, make));
        wait("partial import", &mut |e| match e {
            Event::Built { record, .. } => {
                println!("{}: {}", record.course.title, record.progress());
                assert_eq!(record.chapters.len(), 1);
                assert!(record.locked > 0);
                true
            }
            _ => false,
        });

        // How much of chapter 1 HSK 1–4 cover.
        let known = hanstat::known::Settings {
            levels: vec![1, 2, 3, 4],
            ..Default::default()
        };
        let knowledge = Arc::new(known.apply(None, false).unwrap());
        let shared_root = store::cache_root();
        let l = &record.chapters[0];
        let crd =
            Crd::parse(&std::fs::read(store::crd_path(&shared_root, &l.lesson, &l.crd)).unwrap())
                .unwrap();
        let text = super::super::data::text(&crd, false);
        let inputs = EstimateInputs {
            key: "test".into(),
            knowledge,
            kind: SegmenterKind::Jieba,
            user_words: vec![],
            known_words: known.segmenter_words(),
            cache: None,
            fold_chars: false,
        };
        let pct = known_share(&text, &inputs).unwrap();
        println!("HSK 1–4 know {pct:.1}% of chapter 1");
        assert!((50.0..=100.0).contains(&pct));
    }

    #[test]
    fn free_paths() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            free_path(dir.path(), "A.epub", 3),
            dir.path().join("A.epub")
        );
        std::fs::write(dir.path().join("A.epub"), b"mine").unwrap();
        assert_eq!(
            free_path(dir.path(), "A.epub", 3),
            dir.path().join("A [3].epub")
        );
    }
}
