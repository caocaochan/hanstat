//! The read-aloud machinery, all off the UI thread:
//!
//! - a [`Pool`] of workers asks Azure for sentences, or the local voice for
//!   chunks of paragraphs ([`super::local`]), and saves one clip per
//!   sentence in the audio cache (what playback needs first, pre-generation
//!   after);
//! - the player thread walks the book sentence by sentence (into the next
//!   chapter at the end of one), decodes saved clips and queues them on the
//!   sound device a few seconds ahead, and says which sentence is heard.
//!
//! The UI sends [`Cmd`]s and reads [`Now`].

use super::audio::{self, Output};
use super::azure::{self, Azure, Fail};
use super::{Speech, clip_path, local, opus, saved_clip, sentence_at, sentences};
use crate::reader::tree::Chapter;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub type Chapters = Arc<Mutex<Vec<Option<Arc<Chapter>>>>>;
pub type Repaint = Arc<dyn Fn() + Send + Sync>;

/// Requests to Azure at once.
const WORKERS: usize = 4;
/// Sentences asked for ahead of the one heard.
const AHEAD: usize = 4;
/// Local-voice chunks asked for ahead (each up to its chunk size).
const LOCAL_AHEAD: usize = 3;
/// Seconds of sound kept queued on the device.
const QUEUED_SECONDS: f32 = 6.0;

/// A sentence of a job.
#[derive(Clone)]
struct Clip {
    key: u64,
    /// Where its audio is saved (its book's folder).
    path: PathBuf,
    text: String,
    /// Its paragraph (sentences of one are read run together).
    para: usize,
    /// Not saved yet and not being made elsewhere: this job saves it.
    save: bool,
}

/// What one request makes: a sentence (Azure), or a chunk of paragraphs
/// read at once and cut into its sentences (the local voice). Only clips
/// with `save` are written; the others are there for the reading's flow.
#[derive(Clone)]
struct Job {
    clips: Vec<Clip>,
    speech: Speech,
    batch: Option<Arc<Batch>>,
}

impl Job {
    fn saving(&self) -> impl Iterator<Item = &Clip> {
        self.clips.iter().filter(|c| c.save)
    }

    fn keys(&self) -> Vec<u64> {
        self.saving().map(|c| c.key).collect()
    }
}

/// A pre-generation (a chapter or the book) and how far it got.
#[derive(Default)]
pub struct Batch {
    /// What is generated, for the panel ("this chapter", "the book").
    pub what: String,
    /// Sentences found so far, and whether all chapters were gone through.
    pub total: AtomicUsize,
    pub listed: AtomicBool,
    /// Sentences already saved before, made now, and failed.
    pub cached: AtomicUsize,
    pub done: AtomicUsize,
    pub failed: AtomicUsize,
    /// Characters Azure bills for the sentences not saved before.
    pub billed: AtomicUsize,
    pub cancel: AtomicBool,
    pub error: Mutex<Option<String>>,
}

impl Batch {
    pub fn finished(&self) -> bool {
        self.listed.load(Ordering::Relaxed)
            && self.cached.load(Ordering::Relaxed)
                + self.done.load(Ordering::Relaxed)
                + self.failed.load(Ordering::Relaxed)
                >= self.total.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
struct PoolState {
    urgent: VecDeque<Job>,
    background: VecDeque<Job>,
    /// Keys queued or being made.
    busy: HashSet<u64>,
    failed: HashMap<u64, Fail>,
    /// Azure said to slow down: nothing is sent before this.
    wait_until: Option<Instant>,
    /// The next wait after a 429 without Retry-After.
    backoff: Duration,
}

struct PoolInner {
    state: Mutex<PoolState>,
    cv: Condvar,
    client: Mutex<Option<Arc<Azure>>>,
    stop: AtomicBool,
    repaint: Repaint,
}

/// Workers asking Azure for sentences.
pub struct Pool {
    inner: Arc<PoolInner>,
}

impl Pool {
    fn new(repaint: Repaint) -> Pool {
        let inner = Arc::new(PoolInner {
            state: Mutex::new(PoolState {
                backoff: Duration::from_secs(2),
                ..PoolState::default()
            }),
            cv: Condvar::new(),
            client: Mutex::new(None),
            stop: AtomicBool::new(false),
            repaint,
        });
        for i in 0..WORKERS {
            let inner = inner.clone();
            let _ = std::thread::Builder::new()
                .name(format!("tts-{i}"))
                .spawn(move || worker(&inner));
        }
        Pool { inner }
    }

    fn set_client(&self, c: Option<Arc<Azure>>) {
        *self.inner.client.lock().unwrap() = c;
        // Failures may have been the old key's.
        self.inner.state.lock().unwrap().failed.clear();
    }

    /// Make these first, in this order (sentences about to be heard).
    fn want(&self, jobs: Vec<Job>) {
        let mut s = self.inner.state.lock().unwrap();
        let keys: HashSet<u64> = jobs.iter().flat_map(Job::keys).collect();
        // Queued background jobs with these sentences: those sentences are
        // made here now (a chunk keeps its others).
        for j in s.background.iter_mut() {
            let batch = j.batch.clone();
            for c in j
                .clips
                .iter_mut()
                .filter(|c| c.save && keys.contains(&c.key))
            {
                c.save = false;
                if let Some(b) = &batch {
                    b.done.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        s.background.retain(|j| j.saving().next().is_some());
        // Older urgent jobs are for sentences skipped since: drop them.
        let stale: Vec<Job> = s.urgent.drain(..).collect();
        for j in stale {
            for k in j.keys() {
                s.busy.remove(&k);
            }
        }
        for j in jobs {
            for k in j.keys() {
                s.failed.remove(&k);
                s.busy.insert(k);
            }
            s.urgent.push_back(j);
        }
        self.inner.cv.notify_all();
    }

    fn failure(&self, key: u64) -> Option<Fail> {
        self.inner.state.lock().unwrap().failed.get(&key).cloned()
    }

    fn busy(&self, key: u64) -> bool {
        self.inner.state.lock().unwrap().busy.contains(&key)
    }
}

impl PoolInner {
    fn background(&self, jobs: Vec<Job>) {
        let mut s = self.state.lock().unwrap();
        for mut j in jobs {
            let batch = j.batch.clone();
            for c in j.clips.iter_mut().filter(|c| c.save) {
                if !s.busy.insert(c.key) {
                    // Already being made for playback: counted as made.
                    c.save = false;
                    if let Some(b) = &batch {
                        b.done.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            if j.saving().next().is_some() {
                s.background.push_back(j);
            }
        }
        self.cv.notify_all();
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Relaxed);
        self.inner.cv.notify_all();
    }
}

fn worker(p: &PoolInner) {
    loop {
        let job = {
            let mut s = p.state.lock().unwrap();
            loop {
                if p.stop.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(t) = s.wait_until {
                    let now = Instant::now();
                    if now < t {
                        s = p.cv.wait_timeout(s, t - now).unwrap().0;
                        continue;
                    }
                    s.wait_until = None;
                }
                if let Some(j) = s.urgent.pop_front().or_else(|| s.background.pop_front()) {
                    break j;
                }
                s = p.cv.wait(s).unwrap();
            }
        };
        if let Some(b) = &job.batch
            && b.cancel.load(Ordering::Relaxed)
        {
            let mut s = p.state.lock().unwrap();
            for k in job.keys() {
                s.busy.remove(&k);
            }
            continue;
        }
        let count = job.saving().count();
        let result = if job.saving().all(|c| c.path.exists()) {
            Ok(())
        } else if let Some(l) = &job.speech.local {
            make_local(l, &job)
        } else {
            let client = p.client.lock().unwrap().clone();
            match client {
                None => Err(Fail::Refused("Add your Azure key first".to_owned())),
                Some(c) => make(&c, &job.clips[0], &job.speech),
            }
        };
        let mut s = p.state.lock().unwrap();
        match result {
            Err(Fail::Throttled(after)) => {
                // Everyone waits; this one goes back where it was.
                let wait = after
                    .unwrap_or(s.backoff)
                    .clamp(Duration::from_secs(1), Duration::from_secs(60));
                s.backoff = (s.backoff * 2).min(Duration::from_secs(30));
                s.wait_until = Some(Instant::now() + wait);
                if job.batch.is_some() {
                    s.background.push_front(job);
                } else {
                    s.urgent.push_front(job);
                }
                continue;
            }
            Ok(()) => {
                s.backoff = Duration::from_secs(2);
                if let Some(b) = &job.batch {
                    b.done.fetch_add(count, Ordering::Relaxed);
                }
            }
            Err(e) => {
                if let Some(b) = &job.batch {
                    b.failed.fetch_add(count, Ordering::Relaxed);
                    let mut err = b.error.lock().unwrap();
                    if err.is_none() {
                        *err = Some(e.to_string());
                    }
                    // A refused key fails every sentence: stop.
                    if matches!(e, Fail::Refused(_)) {
                        b.cancel.store(true, Ordering::Relaxed);
                    }
                }
                for k in job.keys() {
                    s.failed.insert(k, e.clone());
                }
            }
        }
        for k in job.keys() {
            s.busy.remove(&k);
        }
        drop(s);
        (p.repaint)();
    }
}

/// Ask Azure for `clip` (a second and third try on network errors) and save it.
fn make(c: &Azure, clip: &Clip, speech: &Speech) -> Result<(), Fail> {
    let mut tries = 0;
    let audio = loop {
        match c.synth(&clip.text, &speech.voice, speech.rate) {
            Ok(a) => break a,
            Err(Fail::Other(_)) if tries < 2 => {
                tries += 1;
                std::thread::sleep(Duration::from_secs(tries));
            }
            Err(e) => return Err(e),
        }
    };
    save(&clip.path, &audio)
}

/// Have the local voice read `job`'s chunk and save its sentences' clips.
fn make_local(l: &local::LocalSettings, job: &Job) -> Result<(), Fail> {
    let texts: Vec<String> = job.clips.iter().map(|c| c.text.clone()).collect();
    let paras: Vec<usize> = job.clips.iter().map(|c| c.para).collect();
    let pcm = local::make_chunk(&texts, &paras, l)?;
    for (c, pcm) in job.clips.iter().zip(pcm) {
        if c.save && !c.path.exists() {
            let file = opus::encode(&pcm).map_err(Fail::Other)?;
            save(&c.path, &file)?;
        }
    }
    Ok(())
}

/// Write `bytes` to `path` (via a temporary name: a clip is whole or absent).
fn save(path: &std::path::Path, bytes: &[u8]) -> Result<(), Fail> {
    let save = || -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    };
    save().map_err(|e| Fail::Other(format!("The audio can't be saved: {e}")))
}

/// What the UI asks the player.
pub enum Cmd {
    /// Read from the sentence holding (or after) character `src` of chapter `spine`.
    Play {
        spine: usize,
        src: u32,
    },
    Pause,
    Resume,
    Stop,
    Speech(Speech),
    Client(Option<Arc<Azure>>),
    Pregenerate(Vec<usize>, Arc<Batch>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Stopped,
    /// Waiting for a sentence's audio.
    Loading,
    Playing,
    Paused,
}

/// What the player is doing, for the UI.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Now {
    pub state: State,
    /// The sentence heard (or waited for): chapter and its characters.
    pub at: Option<(usize, Range<u32>)>,
    pub error: Option<String>,
}

pub struct Engine {
    tx: Sender<Cmd>,
    now: Arc<Mutex<Now>>,
}

impl Engine {
    /// `dir`: the book's folder of saved audio.
    pub fn start(chapters: Chapters, dir: PathBuf, speech: Speech, repaint: Repaint) -> Engine {
        let (tx, rx) = channel();
        let now = Arc::new(Mutex::new(Now::default()));
        {
            let now = now.clone();
            let _ = std::thread::Builder::new()
                .name("tts-player".into())
                .spawn(move || {
                    let mut p = Player::new(chapters, dir, speech, now, repaint);
                    p.run(rx);
                });
        }
        Engine { tx, now }
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    pub fn now(&self) -> Now {
        self.now.lock().unwrap().clone()
    }
}

/// A clip on the device: its sentence.
struct Queued {
    spine: usize,
    idx: usize,
}

struct Player {
    chapters: Chapters,
    /// The book's folder of saved audio.
    dir: PathBuf,
    speech: Speech,
    now: Arc<Mutex<Now>>,
    repaint: Repaint,
    pool: Pool,
    output: Option<Output>,
    state: State,
    /// The next sentence to queue: chapter and index in its sentences.
    cursor: Option<(usize, usize)>,
    /// Sentences on the device, by clip id.
    queued: HashMap<u64, Queued>,
    next_id: u64,
    /// The last sentence heard (kept while paused or loading).
    heard: Option<(usize, usize)>,
    /// The cursor the urgent requests were made for.
    asked: Option<(usize, usize, Speech)>,
    error: Option<String>,
    sentences: HashMap<usize, Arc<Vec<Range<u32>>>>,
    paras: HashMap<usize, Arc<Vec<usize>>>,
    /// Stopped: the device closes at this time, once the sound faded out.
    closing: Option<Instant>,
}

impl Player {
    fn new(
        chapters: Chapters,
        dir: PathBuf,
        speech: Speech,
        now: Arc<Mutex<Now>>,
        repaint: Repaint,
    ) -> Player {
        Player {
            pool: Pool::new(repaint.clone()),
            chapters,
            dir,
            speech,
            now,
            repaint,
            output: None,
            state: State::Stopped,
            cursor: None,
            queued: HashMap::new(),
            next_id: 1,
            heard: None,
            asked: None,
            error: None,
            sentences: HashMap::new(),
            paras: HashMap::new(),
            closing: None,
        }
    }

    fn chapter(&self, spine: usize) -> Option<Arc<Chapter>> {
        self.chapters.lock().unwrap().get(spine).cloned().flatten()
    }

    fn spines(&self) -> usize {
        self.chapters.lock().unwrap().len()
    }

    /// The sentences of chapter `spine`, once it is built.
    fn sentences(&mut self, spine: usize) -> Option<Arc<Vec<Range<u32>>>> {
        if let Some(s) = self.sentences.get(&spine) {
            return Some(s.clone());
        }
        let ch = self.chapter(spine)?;
        let s = Arc::new(sentences(&ch.chars));
        // Keep a few chapters.
        if self.sentences.len() > 8 {
            self.sentences.clear();
        }
        self.sentences.insert(spine, s.clone());
        Some(s)
    }

    /// The paragraph of each sentence of chapter `spine` (the local voice
    /// reads whole paragraphs).
    fn paras(&mut self, spine: usize) -> Option<Arc<Vec<usize>>> {
        if let Some(p) = self.paras.get(&spine) {
            return Some(p.clone());
        }
        let s = self.sentences(spine)?;
        let ch = self.chapter(spine)?;
        let p = Arc::new(local::paragraphs(&ch.chars, &s));
        if self.paras.len() > 8 {
            self.paras.clear();
        }
        self.paras.insert(spine, p.clone());
        Some(p)
    }

    /// The saved clip of `text` read the current way, if any.
    fn saved(&self, text: &str) -> Option<PathBuf> {
        saved_clip(&self.dir, self.speech.key(text), self.speech.ext())
    }

    fn text(&self, spine: usize, r: &Range<u32>) -> String {
        self.chapter(spine)
            .map(|c| c.chars[r.start as usize..r.end as usize].iter().collect())
            .unwrap_or_default()
    }

    /// Past the end of chapter `spine`'s sentences: the next chapter's first.
    fn normalise(&mut self, mut at: (usize, usize)) -> Option<Option<(usize, usize)>> {
        let n = self.spines();
        loop {
            let s = self.sentences(at.0)?;
            if at.1 < s.len() {
                return Some(Some(at));
            }
            if at.0 + 1 >= n {
                return Some(None);
            }
            at = (at.0 + 1, 0);
        }
    }

    fn run(&mut self, rx: Receiver<Cmd>) {
        loop {
            let tick = if matches!(self.state, State::Loading | State::Playing) {
                Duration::from_millis(30)
            } else {
                Duration::from_millis(250)
            };
            match rx.recv_timeout(tick) {
                Ok(c) => self.command(c),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    // The reader closed: fade out before the device goes.
                    if let Some(o) = &self.output {
                        o.clear();
                        o.set_paused(true);
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    return;
                }
            }
            while let Ok(c) = rx.try_recv() {
                self.command(c);
            }
            self.step();
            self.publish();
        }
    }

    fn open_output(&mut self) -> bool {
        if self.output.as_ref().is_some_and(|o| !o.failed()) {
            return true;
        }
        let repaint = self.repaint.clone();
        match Output::open(Arc::new(move || repaint())) {
            Ok(o) => {
                self.output = Some(o);
                true
            }
            Err(e) => {
                self.output = None;
                self.error = Some(e);
                self.state = State::Stopped;
                false
            }
        }
    }

    fn command(&mut self, c: Cmd) {
        match c {
            Cmd::Play { spine, src } => {
                self.error = None;
                self.closing = None;
                if !self.open_output() {
                    return;
                }
                let o = self.output.as_ref().unwrap();
                o.clear();
                o.set_paused(false);
                self.queued.clear();
                self.asked = None;
                let Some(s) = self.sentences(spine) else {
                    // Not built yet: from its start once it is.
                    self.cursor = Some((spine, 0));
                    self.heard = None;
                    self.state = State::Loading;
                    return;
                };
                let idx = sentence_at(&s, src);
                self.cursor = Some((spine, idx));
                self.heard = None;
                self.state = State::Loading;
            }
            Cmd::Pause => {
                if matches!(self.state, State::Loading | State::Playing) {
                    if let Some(o) = &self.output {
                        o.set_paused(true);
                    }
                    self.state = State::Paused;
                }
            }
            Cmd::Resume => {
                if self.state == State::Paused {
                    if let Some(o) = &self.output {
                        o.set_paused(false);
                    }
                    self.state = State::Loading;
                }
            }
            Cmd::Stop => {
                // Faded out; the device closes once it has.
                if let Some(o) = &self.output {
                    o.clear();
                    self.closing = Some(Instant::now() + Duration::from_millis(150));
                }
                self.queued.clear();
                self.cursor = None;
                self.heard = None;
                self.asked = None;
                self.state = State::Stopped;
            }
            Cmd::Speech(s) => {
                if s != self.speech {
                    self.speech = s;
                    // Heard from the current sentence again in the new voice.
                    if matches!(self.state, State::Loading | State::Playing | State::Paused)
                        && let Some((spine, idx)) = self.heard.or(self.cursor)
                        && let Some(s) = self.sentences(spine)
                        && let Some(r) = s.get(idx)
                    {
                        let paused = self.state == State::Paused;
                        self.command(Cmd::Play {
                            spine,
                            src: r.start,
                        });
                        if paused {
                            self.command(Cmd::Pause);
                        }
                    }
                }
            }
            Cmd::Client(c) => {
                self.pool.set_client(c);
                self.asked = None;
            }
            Cmd::Pregenerate(spines, batch) => self.pregenerate(spines, batch),
        }
    }

    /// List the sentences of `spines` not saved yet and queue them, on a
    /// thread of its own (it waits for chapters still being built).
    fn pregenerate(&mut self, spines: Vec<usize>, batch: Arc<Batch>) {
        let chapters = self.chapters.clone();
        let speech = self.speech.clone();
        let dir = self.dir.clone();
        let pool = self.pool.inner.clone();
        let repaint = self.repaint.clone();
        let _ = std::thread::Builder::new()
            .name("tts-pregenerate".into())
            .spawn(move || {
                for spine in spines {
                    let ch = loop {
                        if batch.cancel.load(Ordering::Relaxed) {
                            break None;
                        }
                        match chapters.lock().unwrap().get(spine).cloned() {
                            Some(Some(c)) => break Some(c),
                            Some(None) => {}
                            None => break None,
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    };
                    let Some(ch) = ch else { break };
                    let sents = sentences(&ch.chars);
                    let paras = local::paragraphs(&ch.chars, &sents);
                    let clips: Vec<Clip> = sents
                        .iter()
                        .zip(&paras)
                        .map(|(r, &para)| {
                            let text: String =
                                ch.chars[r.start as usize..r.end as usize].iter().collect();
                            let key = speech.key(&text);
                            let save = saved_clip(&dir, key, speech.ext()).is_none();
                            batch.total.fetch_add(1, Ordering::Relaxed);
                            if save {
                                batch.billed.fetch_add(
                                    azure::billed(&text, speech.rate),
                                    Ordering::Relaxed,
                                );
                            } else {
                                batch.cached.fetch_add(1, Ordering::Relaxed);
                            }
                            Clip {
                                key,
                                path: clip_path(&dir, key, speech.ext()),
                                text,
                                para,
                                save,
                            }
                        })
                        .collect();
                    // A sentence each (Azure), or chunks of whole paragraphs
                    // (the local voice: saved sentences stay in their chunk
                    // for the flow, and aren't made again).
                    let groups = match &speech.local {
                        None => (0..clips.len()).map(|i| i..i + 1).collect(),
                        Some(l) => {
                            let lens: Vec<usize> = sents.iter().map(|r| r.len()).collect();
                            local::chunks(&lens, &paras, 0, |_| l.chunk_chars, usize::MAX)
                        }
                    };
                    let jobs: Vec<Job> = groups
                        .into_iter()
                        .map(|g| Job {
                            clips: clips[g].to_vec(),
                            speech: speech.clone(),
                            batch: Some(batch.clone()),
                        })
                        .filter(|j| j.saving().next().is_some())
                        .collect();
                    pool.background(jobs);
                    repaint();
                }
                batch.listed.store(true, Ordering::Relaxed);
                repaint();
            });
    }

    fn step(&mut self) {
        if self.state == State::Stopped {
            if self.closing.is_some_and(|t| Instant::now() >= t) {
                self.output = None;
                self.closing = None;
            }
            return;
        }
        if self.output.as_ref().is_some_and(Output::failed) {
            // The device changed: open the default one again and go on
            // from the sentence heard.
            let at = self.heard.or(self.cursor);
            self.output = None;
            if let Some((spine, idx)) = at
                && let Some(r) = self.sentences(spine).and_then(|s| s.get(idx).cloned())
            {
                let paused = self.state == State::Paused;
                self.command(Cmd::Play {
                    spine,
                    src: r.start,
                });
                if paused {
                    self.command(Cmd::Pause);
                }
            }
            return;
        }
        let Some(out) = self.output.take() else {
            self.state = State::Stopped;
            return;
        };
        for id in out.take_finished() {
            if let Some(q) = self.queued.remove(&id) {
                self.heard = Some((q.spine, q.idx));
            }
        }
        if let Some(q) = out.current().and_then(|id| self.queued.get(&id)) {
            self.heard = Some((q.spine, q.idx));
        }
        self.fill(&out);
        let playing = !out.queued().is_empty();
        if self.state != State::Paused {
            self.state = if playing {
                State::Playing
            } else {
                State::Loading
            };
        }
        if !playing && self.cursor.is_none() && self.error.is_none() {
            // The end of the book.
            self.state = State::Stopped;
            self.heard = None;
        }
        if self.error.is_some() && !playing {
            self.state = State::Stopped;
        }
        self.output = Some(out);
        if self.state == State::Stopped {
            self.output = None;
        }
    }

    /// Queue sentences on the device while there is less than a few seconds
    /// ahead and their audio is saved; ask for the next ones.
    fn fill(&mut self, out: &Output) {
        loop {
            let Some(at) = self.cursor else { return };
            let at = match self.normalise(at) {
                None => return, // A chapter not built yet.
                Some(None) => {
                    self.cursor = None;
                    return;
                }
                Some(Some(a)) => a,
            };
            self.cursor = Some(at);
            if out.queued().is_empty() {
                // Nothing playing: the sentence waited for is shown.
                self.heard = Some(at);
            }
            self.ask(at);
            if out.ahead() >= QUEUED_SECONDS || out.queued().len() >= 3 {
                return;
            }
            let s = self.sentences(at.0).unwrap();
            let r = s[at.1].clone();
            let text = self.text(at.0, &r);
            let key = self.speech.key(&text);
            let saved = self.saved(&text);
            if let Some(path) = &saved
                && let Ok(bytes) = std::fs::read(path)
            {
                let decoded = if self.speech.local.is_some() {
                    opus::decode(bytes)
                } else {
                    audio::decode_mp3(bytes)
                };
                match decoded {
                    Ok((samples, rate)) => {
                        let id = self.next_id;
                        self.next_id += 1;
                        out.push(id, audio::resample(&samples, rate, out.rate));
                        self.queued.insert(
                            id,
                            Queued {
                                spine: at.0,
                                idx: at.1,
                            },
                        );
                    }
                    Err(_) => {
                        // A damaged file: made again.
                        let _ = std::fs::remove_file(path);
                        self.asked = None;
                        return;
                    }
                }
                self.cursor = Some((at.0, at.1 + 1));
                continue;
            }
            if let Some(f) = self.pool.failure(key) {
                self.error = Some(f.to_string());
                self.cursor = None;
                return;
            }
            if !self.pool.busy(key) {
                // Lost (a stale request dropped): ask again.
                self.asked = None;
            }
            return;
        }
    }

    /// Ask for the sentence at `at` and the few after it, once per position.
    fn ask(&mut self, at: (usize, usize)) {
        if self
            .asked
            .as_ref()
            .is_some_and(|(s, i, sp)| (*s, *i) == at && *sp == self.speech)
        {
            return;
        }
        self.asked = Some((at.0, at.1, self.speech.clone()));
        if let Some(l) = self.speech.local.clone() {
            self.ask_local(at, &l);
            return;
        }
        let mut jobs = Vec::new();
        let mut pos = Some(at);
        // The next few sentences, saved or not.
        for _ in 0..AHEAD {
            let Some(p) = pos else { break };
            let Some(Some(p)) = self.normalise(p) else {
                break;
            };
            let s = self.sentences(p.0).unwrap();
            let text = self.text(p.0, &s[p.1]);
            if self.saved(&text).is_none() {
                let key = self.speech.key(&text);
                jobs.push(Job {
                    clips: vec![Clip {
                        key,
                        path: clip_path(&self.dir, key, self.speech.ext()),
                        text,
                        para: 0,
                        save: true,
                    }],
                    speech: self.speech.clone(),
                    batch: None,
                });
            }
            pos = Some((p.0, p.1 + 1));
        }
        if !jobs.is_empty() {
            self.pool.want(jobs);
        }
    }

    /// The local voice: from the first sentence at or after `at` that is
    /// neither saved nor being made, a few chunks of whole paragraphs. When
    /// the sentence at `at` itself is missing, the first chunks are short
    /// (a quarter, then half the size), so sound starts within seconds and
    /// each chunk is ready before the one before it has been heard.
    fn ask_local(&mut self, at: (usize, usize), l: &local::LocalSettings) {
        let full = l.chunk_chars.max(20);
        // How far ahead to look, in characters.
        let reach = full * LOCAL_AHEAD;
        let mut jobs = Vec::new();
        let mut cold = None;
        let mut seen = 0;
        let mut pos = at;
        while jobs.len() < LOCAL_AHEAD && seen <= reach {
            let Some(Some(p)) = self.normalise(pos) else {
                break;
            };
            let sents = self.sentences(p.0).unwrap();
            let paras = self.paras(p.0).unwrap();
            // Past what is saved or on its way.
            let mut i = p.1;
            while i < sents.len() && seen <= reach {
                let text = self.text(p.0, &sents[i]);
                if self.saved(&text).is_none() && !self.pool.busy(self.speech.key(&text)) {
                    break;
                }
                cold.get_or_insert(false);
                seen += sents[i].len();
                i += 1;
            }
            if i >= sents.len() || seen > reach {
                pos = (p.0, i);
                continue;
            }
            let cold = *cold.get_or_insert(true);
            let lens: Vec<usize> = sents.iter().map(|r| r.len()).collect();
            let done = jobs.len();
            let limit = |k: usize| {
                if cold {
                    [full / 4, full / 2]
                        .get(done + k)
                        .copied()
                        .unwrap_or(full)
                        .max(20)
                } else {
                    full
                }
            };
            let groups = local::chunks(&lens, &paras, i, limit, LOCAL_AHEAD - jobs.len());
            let mut end = i;
            for g in groups {
                end = g.end;
                seen += lens[g.clone()].iter().sum::<usize>();
                let clips: Vec<Clip> = g
                    .map(|k| {
                        let text = self.text(p.0, &sents[k]);
                        let key = self.speech.key(&text);
                        let save = self.saved(&text).is_none() && !self.pool.busy(key);
                        Clip {
                            key,
                            path: clip_path(&self.dir, key, self.speech.ext()),
                            text,
                            para: paras[k],
                            save,
                        }
                    })
                    .collect();
                if clips.iter().any(|c| c.save) {
                    jobs.push(Job {
                        clips,
                        speech: self.speech.clone(),
                        batch: None,
                    });
                }
            }
            pos = (p.0, end.max(i + 1));
        }
        if !jobs.is_empty() {
            self.pool.want(jobs);
        }
    }

    fn publish(&mut self) {
        let at = self.heard.and_then(|(spine, idx)| {
            self.sentences
                .get(&spine)
                .and_then(|s| s.get(idx).cloned())
                .map(|r| (spine, r))
        });
        let now = Now {
            state: self.state,
            at: if self.state == State::Stopped {
                None
            } else {
                at
            },
            error: self.error.clone(),
        };
        let mut n = self.now.lock().unwrap();
        if *n != now {
            *n = now;
            drop(n);
            (self.repaint)();
        }
    }
}
