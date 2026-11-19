//! Read aloud: the book read sentence by sentence by an Azure neural voice
//! or a voice on this computer (Qwen3-TTS on the GPU), the sentence heard
//! highlighted and the pages turned with it. While it reads, a click on a
//! word reads from its sentence.
//!
//! Every sentence's audio is saved (Azure: an MP3 per sentence; the local
//! voice: Ogg Opus), keyed by voice, settings and text, so a passage is made
//! once; a chapter or the whole book can be made ahead of time. The two
//! voices' clips never share a file.
//!
//! - [`azure`]: the REST API; [`key`]: the key, kept encrypted.
//! - [`local`]: the local voice's server, chunks and cuts; [`opus`]: its
//!   clips; `local_ui`: its settings and the "Generate locally" window.
//! - [`audio`]: decoding and the sound device; [`engine`]: the threads.

pub mod audio;
pub mod azure;
mod engine;
pub mod key;
pub mod local;
mod local_ui;
pub mod opus;

use crate::reader::tree::Chapter;
use eframe::egui::{self, Color32, RichText};
#[cfg(test)]
pub use engine::State as PlayState;
use engine::{Batch, Chapters, Cmd, Engine, Now, Repaint, State};
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

/// Read-aloud settings, kept in the reader's settings file (the key is not:
/// see [`key`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsSettings {
    /// The Azure Speech resource's region (`eastus`, `westeurope`, …).
    pub region: String,
    /// The voice for Simplified and for Traditional books; empty: the default.
    pub voice: String,
    pub voice_trad: String,
    /// Speed change in percent.
    pub rate: i32,
    /// The dictionary popup pauses reading while it is open.
    pub pause_for_popup: bool,
    /// Who reads: Azure or the local voice.
    pub source: Source,
    pub local: local::LocalSettings,
}

/// Who reads aloud.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    #[default]
    Azure,
    /// Qwen3-TTS on this computer ([`local`]).
    Local,
}

impl Default for TtsSettings {
    fn default() -> Self {
        TtsSettings {
            region: String::new(),
            voice: String::new(),
            voice_trad: String::new(),
            rate: 0,
            pause_for_popup: true,
            source: Source::Azure,
            local: local::LocalSettings::default(),
        }
    }
}

impl TtsSettings {
    pub fn voice(&self, traditional: bool) -> String {
        let v = if traditional {
            &self.voice_trad
        } else {
            &self.voice
        };
        if v.is_empty() {
            azure::default_voice(traditional).to_owned()
        } else {
            v.clone()
        }
    }
}

/// How sentences are read: a clip made with one is not used for another.
#[derive(Clone, Debug, PartialEq)]
pub struct Speech {
    pub voice: String,
    pub rate: i32,
    /// The local voice and its settings (`voice` and `rate` unused); `None`: Azure.
    pub local: Option<local::LocalSettings>,
}

impl Speech {
    /// The cache key of `text` read this way.
    pub fn key(&self, text: &str) -> u64 {
        let s = match &self.local {
            None => format!(
                "{}\u{1}{}\u{1}{}\u{1}{text}",
                self.voice,
                self.rate,
                azure::FORMAT
            ),
            Some(l) => format!("{}\u{1}{text}", l.key_text()),
        };
        hanstat::dict::xxh64::xxh64(s.as_bytes(), 0)
    }

    /// Its clips' file extension.
    pub fn ext(&self) -> &'static str {
        if self.local.is_some() { OPUS } else { MP3 }
    }
}

/// The two kinds of saved clip.
pub const MP3: &str = "mp3";
pub const OPUS: &str = "opus";

/// Where saved audio is kept (it can be made again: the cache folder).
pub fn cache_root() -> PathBuf {
    hanstat::models::cache_dir().join("read-aloud")
}

/// The folder of one book's saved audio: its title, to be found in a file
/// manager, and a hash of its Library id, so two books of one title differ.
pub fn book_dir(title: &str, book_id: &str) -> PathBuf {
    let name: String = title
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .take(60)
        .collect();
    let name = name.trim().trim_end_matches('.').trim();
    let hash = format!(
        "{:08x}",
        hanstat::dict::xxh64::xxh64(book_id.as_bytes(), 0) as u32
    );
    cache_root().join(if name.is_empty() {
        hash
    } else {
        format!("{name} [{hash}]")
    })
}

/// A sentence's audio in its book's folder: `ext` is [`MP3`] or [`OPUS`].
pub fn clip_path(dir: &Path, key: u64, ext: &str) -> PathBuf {
    dir.join(format!("{key:016x}.{ext}"))
}

/// Where 2026.11.19.02's build kept audio: 256 folders by the key's first
/// byte, every book's mixed.
fn legacy_path(key: u64) -> PathBuf {
    cache_root()
        .join(format!("{:02x}", key >> 56))
        .join(format!("{key:016x}.mp3"))
}

/// A legacy folder (two hex digits).
fn is_legacy(name: &str) -> bool {
    name.len() == 2 && name.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The saved audio of `key` for the book in `dir`, if any; an MP3 saved in
/// a legacy folder is moved into the book's.
pub fn saved_clip(dir: &Path, key: u64, ext: &str) -> Option<PathBuf> {
    let p = clip_path(dir, key, ext);
    if p.exists() {
        return Some(p);
    }
    if ext != MP3 {
        return None;
    }
    let old = legacy_path(key);
    if !old.exists() {
        return None;
    }
    if std::fs::create_dir_all(dir).is_ok() && std::fs::rename(&old, &p).is_ok() {
        Some(p)
    } else {
        Some(old)
    }
}

/// Saved audio: sentences and bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub files: usize,
    pub bytes: u64,
}

/// Saved audio by voice: Azure's MP3 and the local voice's Opus.
#[derive(Clone, Copy, Debug, Default)]
pub struct Kinds {
    pub azure: Usage,
    pub local: Usage,
}

impl Kinds {
    fn add(&mut self, o: Kinds) {
        for (a, b) in [(&mut self.azure, o.azure), (&mut self.local, o.local)] {
            a.files += b.files;
            a.bytes += b.bytes;
        }
    }

    pub fn of(&self, local: bool) -> Usage {
        if local { self.local } else { self.azure }
    }
}

/// What a folder's clips take, by voice.
fn usage(dir: &Path) -> Kinds {
    let mut u = Kinds::default();
    if let Ok(files) = std::fs::read_dir(dir) {
        for f in files.flatten() {
            let p = f.path();
            let kind = match p.extension().and_then(|e| e.to_str()) {
                Some(MP3) => &mut u.azure,
                Some(OPUS) => &mut u.local,
                _ => continue,
            };
            if let Ok(m) = f.metadata()
                && m.is_file()
            {
                kind.files += 1;
                kind.bytes += m.len();
            }
        }
    }
    u
}

/// This book's saved audio and every book's; empty legacy folders go.
fn measure(dir: &Path) -> (Kinds, Kinds) {
    let book = usage(dir);
    let mut all = Kinds::default();
    if let Ok(dirs) = std::fs::read_dir(cache_root()) {
        for d in dirs.flatten() {
            if !d.path().is_dir() {
                continue;
            }
            let u = usage(&d.path());
            if u.azure.files + u.local.files == 0 && is_legacy(&d.file_name().to_string_lossy()) {
                let _ = std::fs::remove_dir(d.path());
            }
            all.add(u);
        }
    }
    (book, all)
}

/// Delete one voice's clips in `dir` (the other voice's stay); the folder
/// goes once empty.
fn delete_clips(dir: &Path, ext: &str) {
    if let Ok(files) = std::fs::read_dir(dir) {
        for f in files.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) == Some(ext) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// Past this many characters a sentence is split at a comma, past
/// [`HARD`] anywhere (a request each, and the highlight follows).
const SOFT: usize = 120;
const HARD: usize = 300;

/// The sentences of a chapter's characters ([`Chapter::chars`]): split at
/// sentence ends (closing quotes kept with them), block ends and line
/// breaks; without the spaces around them; only those with a letter, digit
/// or Chinese character to read.
pub fn sentences(chars: &[char]) -> Vec<Range<u32>> {
    use crate::reader::anki::{closer, ends};
    let mut out = Vec::new();
    let push = |out: &mut Vec<Range<u32>>, mut a: usize, mut b: usize| {
        let blank = |c: char| c.is_whitespace() || c == '\0';
        while a < b && blank(chars[a]) {
            a += 1;
        }
        while b > a && blank(chars[b - 1]) {
            b -= 1;
        }
        if chars[a..b].iter().any(|c| c.is_alphanumeric()) {
            out.push(a as u32..b as u32);
        }
    };
    let (n, mut start, mut i) = (chars.len(), 0, 0);
    while i < n {
        let c = chars[i];
        if c == '\0' || c == '\n' {
            push(&mut out, start, i);
            i += 1;
            start = i;
            continue;
        }
        i += 1;
        let len = i - start;
        if ends(c) {
            while i < n && (ends(chars[i]) || closer(chars[i])) {
                i += 1;
            }
            push(&mut out, start, i);
            start = i;
        } else if (len >= SOFT && matches!(c, '，' | '、' | '：' | ',' | ':' | '—' | '…'))
            || len >= HARD
        {
            push(&mut out, start, i);
            start = i;
        }
    }
    push(&mut out, start, n);
    out
}

/// The sentence holding character `src`, or the first after it (`len` past
/// the last).
pub fn sentence_at(s: &[Range<u32>], src: u32) -> usize {
    s.partition_point(|r| r.end <= src)
}

/// What the panel asks the reader to do (it knows the pages).
pub enum Action {
    /// Read from the first sentence of the page shown.
    PlayPage,
}

enum KeyState {
    /// Not looked for yet (looking may ask to unlock the keyring).
    Unknown,
    Loading(Receiver<Result<Option<String>, String>>),
    Loaded(Option<String>),
}

enum Voices {
    None,
    Loading(Receiver<Result<Vec<azure::Voice>, String>>),
    Ready(Vec<azure::Voice>),
    Failed(String),
}

/// The read-aloud state in the reader.
pub struct Tts {
    chapters: Chapters,
    repaint: Repaint,
    traditional: bool,
    engine: Option<Engine>,
    now: Now,
    /// The Aa-style panel is open.
    pub panel_open: bool,
    key: KeyState,
    /// The key being typed, and whether the field shows.
    key_edit: String,
    key_editing: bool,
    key_note: Option<(bool, String)>,
    key_saving: Option<Receiver<Result<(), String>>>,
    voices: Voices,
    voices_region: String,
    /// What the engine was last told.
    sent_speech: Option<Speech>,
    sent_client: Option<(String, String)>,
    /// Read when the key is known (asked before it was loaded).
    pending: Option<(usize, u32)>,
    batch: Option<Arc<Batch>>,
    /// This book's folder of saved audio.
    dir: PathBuf,
    /// Saved audio: this book's and every book's.
    cache_size: Option<(Kinds, Kinds)>,
    cache_job: Option<Receiver<(Kinds, Kinds)>>,
    /// Each chapter's name in the table of contents (empty: none).
    labels: Vec<String>,
    /// The first character of the page shown (the preview reads from it).
    pub page_src: u32,
    /// The local voice's panel and window.
    local: local_ui::State,
    /// The chapter shown (the preview reads from it).
    local_spine: usize,
    /// Sentences and billed characters: per chapter, and the book's.
    estimates: std::collections::HashMap<(usize, i32), (usize, usize)>,
    book_estimate: Option<(i32, usize, usize)>,
    /// The sentence heard last frame and whether it was on the pages shown
    /// (the pages follow it only then).
    pub last_at: Option<(usize, u32)>,
    pub last_visible: bool,
    /// The dictionary popup was open last frame, and paused the reading.
    popup_open: bool,
    paused_by_popup: bool,
}

impl Tts {
    pub fn new(
        chapters: Arc<Mutex<Vec<Option<Arc<Chapter>>>>>,
        labels: Vec<String>,
        traditional: bool,
        title: &str,
        book_id: &str,
        ctx: &egui::Context,
    ) -> Tts {
        let ctx = ctx.clone();
        Tts {
            dir: book_dir(title, book_id),
            chapters,
            repaint: Arc::new(move || ctx.request_repaint()),
            traditional,
            engine: None,
            now: Now::default(),
            panel_open: false,
            key: KeyState::Unknown,
            key_edit: String::new(),
            key_editing: false,
            key_note: None,
            key_saving: None,
            voices: Voices::None,
            voices_region: String::new(),
            sent_speech: None,
            sent_client: None,
            pending: None,
            batch: None,
            cache_size: None,
            cache_job: None,
            labels,
            page_src: 0,
            local: local_ui::State::default(),
            local_spine: 0,
            estimates: Default::default(),
            book_estimate: None,
            last_at: None,
            last_visible: false,
            popup_open: false,
            paused_by_popup: false,
        }
    }

    fn speech(&self, s: &TtsSettings) -> Speech {
        match s.source {
            Source::Azure => Speech {
                voice: s.voice(self.traditional),
                rate: s.rate,
                local: None,
            },
            Source::Local => Speech {
                voice: String::new(),
                rate: 0,
                local: Some(s.local.clone()),
            },
        }
    }

    fn the_key(&self) -> Option<&str> {
        match &self.key {
            KeyState::Loaded(Some(k)) => Some(k),
            _ => None,
        }
    }

    fn load_key(&mut self) {
        if matches!(self.key, KeyState::Unknown) {
            let (tx, rx) = channel();
            let repaint = self.repaint.clone();
            let _ = std::thread::Builder::new()
                .name("tts-key".into())
                .spawn(move || {
                    let _ = tx.send(key::load());
                    repaint();
                });
            self.key = KeyState::Loading(rx);
        }
    }

    fn engine(&mut self, s: &TtsSettings) -> &Engine {
        if self.engine.is_none() {
            self.engine = Some(Engine::start(
                self.chapters.clone(),
                self.dir.clone(),
                self.speech(s),
                self.repaint.clone(),
            ));
            self.sent_speech = Some(self.speech(s));
            self.sent_client = None;
        }
        self.engine.as_ref().unwrap()
    }

    /// Once a frame: answers from the threads, and the engine told about
    /// changed settings.
    pub fn poll(&mut self, s: &TtsSettings) {
        self.local_poll();
        if let KeyState::Loading(rx) = &self.key
            && let Ok(r) = rx.try_recv()
        {
            self.key = match r {
                Ok(k) => KeyState::Loaded(k),
                Err(e) => {
                    self.key_note = Some((false, e));
                    KeyState::Loaded(None)
                }
            };
        }
        if let Some(rx) = &self.key_saving
            && let Ok(r) = rx.try_recv()
        {
            self.key_saving = None;
            self.key_note = Some(match r {
                Ok(()) if self.the_key().is_some() => {
                    (true, format!("Saved, {}.", key::where_kept()))
                }
                Ok(()) => (true, "Removed.".to_owned()),
                Err(e) => (false, format!("{e}. The key is used until hanstat closes.")),
            });
        }
        if let Voices::Loading(rx) = &self.voices
            && let Ok(r) = rx.try_recv()
        {
            self.voices = match r {
                Ok(v) => Voices::Ready(v),
                Err(e) => Voices::Failed(e),
            };
        }
        if let Some(rx) = &self.cache_job
            && let Ok(r) = rx.try_recv()
        {
            self.cache_size = Some(r);
            self.cache_job = None;
        }
        let Some(engine) = &self.engine else { return };
        self.now = engine.now();
        let speech = self.speech(s);
        if self.sent_speech.as_ref() != Some(&speech) {
            engine.send(Cmd::Speech(speech.clone()));
            self.sent_speech = Some(speech);
        }
        let client = self
            .the_key()
            .map(|k| (s.region.trim().to_owned(), k.to_owned()));
        if client != self.sent_client {
            engine.send(Cmd::Client(
                client
                    .as_ref()
                    .map(|(r, k)| Arc::new(azure::Azure::new(r, k))),
            ));
            self.sent_client = client;
        }
        if self.pending.is_some()
            && s.source == Source::Azure
            && matches!(self.key, KeyState::Loaded(_))
        {
            let pending = self.pending.take();
            if self.client_ready(s) {
                if let (Some((spine, src)), Some(engine)) = (pending, &self.engine) {
                    engine.send(Cmd::Play { spine, src });
                    // Shown as loading until the player answers.
                    self.now.state = State::Loading;
                }
            } else {
                self.panel_open = true;
                self.key_note = Some((
                    false,
                    "Add your Azure Speech key and its region to read aloud.".to_owned(),
                ));
            }
        }
    }

    fn client_ready(&self, s: &TtsSettings) -> bool {
        match s.source {
            Source::Azure => self.the_key().is_some() && azure::valid_region(s.region.trim()),
            Source::Local => self.local.downloaded(),
        }
    }

    pub fn state(&self) -> State {
        if self.pending.is_some() {
            State::Loading
        } else {
            self.now.state
        }
    }

    /// Reading aloud (or paused, or waiting for audio).
    pub fn active(&self) -> bool {
        self.state() != State::Stopped
    }

    /// The sentence heard: its chapter and characters.
    pub fn at(&self) -> Option<(usize, Range<u32>)> {
        self.now.at.clone()
    }

    /// Read from the sentence holding character `src` of chapter `spine`.
    pub fn play_from(&mut self, spine: usize, src: u32, s: &TtsSettings) {
        self.paused_by_popup = false;
        if s.source == Source::Local {
            self.last_visible = true;
            self.last_at = None;
            self.pending = None;
            if !self.local.downloaded() {
                self.panel_open = true;
                self.key_note = Some((false, local::NOT_DOWNLOADED.to_owned()));
                return;
            }
            self.stop_preview();
            self.engine(s);
            // The engine knows the current settings before it plays.
            self.poll(s);
            if let Some(e) = &self.engine {
                e.send(Cmd::Play { spine, src });
            }
            self.now.state = State::Loading;
            return;
        }
        self.load_key();
        self.engine(s);
        self.last_visible = true;
        self.last_at = None;
        if !matches!(self.key, KeyState::Loading(_)) && !self.client_ready(s) {
            // Nothing to read with yet: the panel says what is missing.
            self.panel_open = true;
            self.pending = None;
            self.key_note = Some((
                false,
                "Add your Azure Speech key and its region to read aloud.".to_owned(),
            ));
            return;
        }
        // Sent once the key is loaded and the engine knows it.
        self.pending = Some((spine, src));
        self.poll(s);
    }

    /// Once a frame, with whether the dictionary popup is open: it pauses
    /// the reading when it opens and resumes it when it closes (if set).
    pub fn popup(&mut self, open: bool, s: &TtsSettings) {
        let was = std::mem::replace(&mut self.popup_open, open);
        if open && !was {
            if s.pause_for_popup
                && matches!(self.now.state, State::Loading | State::Playing)
                && let Some(e) = &self.engine
            {
                e.send(Cmd::Pause);
                self.now.state = State::Paused;
                self.paused_by_popup = true;
            }
        } else if !open
            && was
            && std::mem::take(&mut self.paused_by_popup)
            && self.now.state == State::Paused
            && let Some(e) = &self.engine
        {
            e.send(Cmd::Resume);
            self.now.state = State::Loading;
        }
    }

    pub fn toggle_pause(&mut self) {
        // Paused or resumed by hand: the popup leaves it so.
        self.paused_by_popup = false;
        if let Some(e) = &self.engine {
            match self.now.state {
                State::Paused => e.send(Cmd::Resume),
                State::Loading | State::Playing => e.send(Cmd::Pause),
                State::Stopped => {}
            }
        }
    }

    pub fn stop(&mut self) {
        self.pending = None;
        self.paused_by_popup = false;
        if let Some(e) = &self.engine {
            e.send(Cmd::Stop);
        }
        self.now.state = State::Stopped;
        self.now.at = None;
    }

    fn fetch_voices(&mut self, s: &TtsSettings, refresh: bool) {
        let region = s.region.trim().to_owned();
        let Some(key) = self.the_key().map(str::to_owned) else {
            return;
        };
        if !azure::valid_region(&region) {
            return;
        }
        let file = cache_root().join(format!("voices-{region}.json"));
        if !refresh
            && let Ok(b) = std::fs::read(&file)
            && let Ok(v) = serde_json::from_slice::<Vec<azure::Voice>>(&b)
        {
            self.voices = Voices::Ready(azure::chinese_voices(v));
            self.voices_region = region;
            return;
        }
        let (tx, rx) = channel();
        let repaint = self.repaint.clone();
        self.voices_region = region.clone();
        let _ = std::thread::Builder::new()
            .name("tts-voices".into())
            .spawn(move || {
                let r = azure::Azure::new(&region, &key)
                    .voices()
                    .map_err(|e| e.to_string())
                    .map(|all| {
                        let _ = std::fs::create_dir_all(cache_root());
                        let _ = std::fs::write(&file, serde_json::to_vec(&all).unwrap_or_default());
                        azure::chinese_voices(all)
                    });
                let _ = tx.send(r);
                repaint();
            });
        self.voices = Voices::Loading(rx);
    }

    /// Measure the saved audio (this book's and all), and remove the
    /// legacy folders left empty.
    fn measure_cache(&mut self) {
        if self.cache_job.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let repaint = self.repaint.clone();
        let dir = self.dir.clone();
        let _ = std::thread::Builder::new()
            .name("tts-cache".into())
            .spawn(move || {
                let _ = tx.send(measure(&dir));
                repaint();
            });
        self.cache_job = Some(rx);
    }

    /// Delete one voice's saved audio (`ext`: [`MP3`] or [`OPUS`]), this
    /// book's or every book's; the other voice's stays.
    fn delete_cache(&mut self, all: bool, ext: &'static str) {
        let (tx, rx) = channel();
        let repaint = self.repaint.clone();
        let dir = self.dir.clone();
        let _ = std::thread::Builder::new()
            .name("tts-cache".into())
            .spawn(move || {
                if all {
                    if let Ok(dirs) = std::fs::read_dir(cache_root()) {
                        for d in dirs.flatten() {
                            // The audio folders; the voice lists stay.
                            if d.path().is_dir() {
                                delete_clips(&d.path(), ext);
                            }
                        }
                    }
                } else {
                    delete_clips(&dir, ext);
                }
                let _ = tx.send(measure(&dir));
                repaint();
            });
        self.cache_job = Some(rx);
        self.cache_size = None;
    }

    /// Sentences and billed characters of chapter `spine`.
    fn estimate(&mut self, spine: usize, rate: i32) -> Option<(usize, usize)> {
        if let Some(e) = self.estimates.get(&(spine, rate)) {
            return Some(*e);
        }
        let ch = self
            .chapters
            .lock()
            .unwrap()
            .get(spine)
            .cloned()
            .flatten()?;
        let s = sentences(&ch.chars);
        let billed = s
            .iter()
            .map(|r| {
                let t: String = ch.chars[r.start as usize..r.end as usize].iter().collect();
                azure::billed(&t, rate)
            })
            .sum();
        let e = (s.len(), billed);
        self.estimates.insert((spine, rate), e);
        Some(e)
    }

    /// The book's, once every chapter is built.
    fn book_estimate(&mut self, rate: i32) -> Option<(usize, usize)> {
        if let Some((r, n, b)) = self.book_estimate
            && r == rate
        {
            return Some((n, b));
        }
        let n = self.chapters.lock().unwrap().len();
        if self.chapters.lock().unwrap().iter().any(Option::is_none) {
            return None;
        }
        let (mut sent, mut billed) = (0, 0);
        for i in 0..n {
            let (s, b) = self.estimate(i, rate)?;
            sent += s;
            billed += b;
        }
        self.book_estimate = Some((rate, sent, billed));
        Some((sent, billed))
    }

    /// Use `key` for this session only (tests: nothing is stored).
    #[cfg(test)]
    pub fn use_key(&mut self, key: &str) {
        self.key = KeyState::Loaded(Some(key.to_owned()));
    }

    #[cfg(test)]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    #[cfg(test)]
    pub fn batch(&self) -> Option<Arc<Batch>> {
        self.batch.clone()
    }

    pub(super) fn pregenerate(&mut self, s: &TtsSettings, spines: Vec<usize>, what: &str) {
        let batch = Arc::new(Batch {
            what: what.to_owned(),
            ..Batch::default()
        });
        self.engine(s).send(Cmd::Pregenerate(spines, batch.clone()));
        self.batch = Some(batch);
        self.cache_size = None;
    }

    /// The controls in the reader's bar: the panel's button, and pause and
    /// stop while reading.
    pub fn bar(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let b = ui
            .selectable_label(self.panel_open, RichText::new("Read aloud").size(14.0))
            .on_hover_text("Have the book read to you by an Azure voice or one on this computer");
        if b.clicked() {
            self.panel_open = !self.panel_open;
            if self.panel_open {
                self.load_key();
            }
        }
        if self.active() {
            if ui
                .button(RichText::new("Stop").size(14.0))
                .on_hover_text("Stop reading aloud")
                .clicked()
            {
                self.stop();
            }
            let paused = self.state() == State::Paused;
            if ui
                .button(RichText::new(if paused { "Resume" } else { "Pause" }).size(14.0))
                .clicked()
            {
                self.toggle_pause();
            }
        }
        b
    }

    /// The panel under the bar's button: playback, voice, speed, the Azure
    /// key, making audio ahead of time and the saved audio.
    pub fn panel(
        &mut self,
        button: &egui::Response,
        s: &mut TtsSettings,
        spine: usize,
    ) -> Option<Action> {
        let mut action = None;
        let mut close = false;
        let ctx = button.ctx.clone();
        let mut open = self.panel_open;
        if open {
            self.load_key();
        }
        let ready = self.client_ready(s);
        if open
            && ready
            && (matches!(self.voices, Voices::None) || self.voices_region != s.region.trim())
        {
            self.fetch_voices(s, false);
        }
        if open && self.cache_size.is_none() {
            self.measure_cache();
        }
        egui::Popup::menu(button)
            .id(egui::Id::unique("reader-tts"))
            .open_bool(&mut open)
            .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
            .show(|ui| {
                ui.set_min_width(380.0);
                ui.set_max_width(440.0);
                let max_h = ui.ctx().content_rect().height() - 120.0;
                egui::ScrollArea::vertical().max_height(max_h.max(200.0)).show(ui, |ui| {
                // Playback.
                ui.horizontal(|ui| {
                    match self.state() {
                        State::Stopped => {
                            let can = match s.source {
                                Source::Azure => ready || matches!(self.key, KeyState::Loading(_) | KeyState::Unknown),
                                Source::Local => ready,
                            };
                            if ui.add_enabled(can, egui::Button::new("Read from this page")).clicked() {
                                action = Some(Action::PlayPage);
                            }
                        }
                        st => {
                            let label = if st == State::Paused { "Resume" } else { "Pause" };
                            if ui.button(label).clicked() {
                                self.toggle_pause();
                            }
                            if ui.button("Stop").clicked() {
                                self.stop();
                            }
                            ui.label(
                                RichText::new(match st {
                                    State::Loading if s.source == Source::Local => {
                                        if local::status() == local::Status::Loaded {
                                            "Making the audio…"
                                        } else {
                                            "Loading the voice…"
                                        }
                                    }
                                    State::Loading => "Waiting for Azure…",
                                    State::Paused => "Paused",
                                    _ => "Reading",
                                })
                                .weak(),
                            );
                        }
                    }
                });
                ui.label(
                    RichText::new("While reading, click a word to read from its sentence.")
                        .size(12.0)
                        .weak(),
                );
                if let Some(e) = &self.now.error {
                    ui.colored_label(Color32::from_rgb(0xdc, 0x32, 0x2f), e);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Read by");
                    ui.selectable_value(&mut s.source, Source::Azure, "Azure")
                        .on_hover_text("Microsoft's neural voices, over the internet (your key)");
                    ui.selectable_value(&mut s.source, Source::Local, "This computer")
                        .on_hover_text("Qwen3-TTS on your GPU: free and offline, after a one-time download");
                });
                if s.source == Source::Local {
                    self.local_settings(ui, s);
                } else {
                egui::Grid::new("reader-tts-grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Voice");
                        self.voice_choice(ui, s);
                        ui.end_row();
                        ui.label("Speed");
                        ui.add(
                            egui::Slider::new(&mut s.rate, -50..=100)
                                .step_by(5.0)
                                .custom_formatter(|v, _| {
                                    if v == 0.0 { "normal".to_owned() } else { format!("{v:+.0} %") }
                                }),
                        )
                        .on_hover_text("Each speed is its own audio: changing it makes sentences again");
                        ui.end_row();
                        ui.label("Dictionary");
                        ui.checkbox(&mut s.pause_for_popup, "Pause while the popup is open")
                            .on_hover_text("Reading stops while you look a word up and goes on when the popup closes");
                        ui.end_row();
                        ui.label("Region");
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut s.region)
                                .hint_text("e.g. westeurope")
                                .desired_width(200.0),
                        );
                        if r.changed() {
                            s.region = s.region.trim().to_ascii_lowercase();
                        }
                        ui.end_row();
                        ui.label("Azure key");
                        self.key_field(ui);
                        ui.end_row();
                    });
                if !s.region.is_empty() && !azure::valid_region(&s.region) {
                    ui.colored_label(
                        Color32::from_rgb(0xdc, 0x32, 0x2f),
                        "A region is a name such as eastus or westeurope.",
                    );
                }
                if let Some((ok, note)) = &self.key_note {
                    let c = if *ok { ui.visuals().weak_text_color() } else { Color32::from_rgb(0xdc, 0x32, 0x2f) };
                    ui.colored_label(c, note);
                }
                ui.label(
                    RichText::new(
                        "The key and region are on your Speech resource's \
                         “Keys and Endpoint” page in the Azure portal.",
                    )
                    .size(12.0)
                    .weak(),
                );
                }
                ui.separator();
                if self.ahead_of_time(ui, s, spine, ready) {
                    close = true;
                }
                });
            });
        if close {
            open = false;
        }
        self.panel_open = open;
        self.local_window(&ctx, s, spine);
        action
    }

    /// The pre-generation under way or last finished, with Cancel or Close.
    fn batch_progress(&mut self, ui: &mut egui::Ui) {
        if let Some(b) = self.batch.clone() {
            let total = b.total.load(Ordering::Relaxed);
            let cached = b.cached.load(Ordering::Relaxed);
            let done = b.done.load(Ordering::Relaxed);
            let failed = b.failed.load(Ordering::Relaxed);
            let listed = b.listed.load(Ordering::Relaxed);
            let cancelled = b.cancel.load(Ordering::Relaxed);
            let finished = b.finished();
            let frac = if total == 0 {
                0.0
            } else {
                (cached + done + failed) as f32 / total as f32
            };
            ui.horizontal(|ui| {
                let text = format!(
                    "{}: {} of {}{}{}",
                    b.what,
                    group(cached + done),
                    group(total),
                    if listed { "" } else { "+" },
                    if failed > 0 {
                        format!(", {} failed", group(failed))
                    } else {
                        String::new()
                    }
                );
                ui.add(egui::ProgressBar::new(frac).desired_width(220.0).text(text));
                if !finished && !cancelled {
                    if ui.small_button("Cancel").clicked() {
                        b.cancel.store(true, Ordering::Relaxed);
                    }
                } else if ui.small_button("Close").clicked() {
                    self.batch = None;
                    self.cache_size = None;
                }
            });
            if cancelled && !finished {
                ui.label(RichText::new("Cancelled.").weak());
            }
            if let Some(e) = b.error.lock().unwrap().as_ref() {
                ui.colored_label(Color32::from_rgb(0xdc, 0x32, 0x2f), e);
            }
            if !finished && !cancelled {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
    }

    fn voice_choice(&mut self, ui: &mut egui::Ui, s: &mut TtsSettings) {
        let current = s.voice(self.traditional);
        ui.horizontal(|ui| {
            let shown = match &self.voices {
                Voices::Ready(v) => v
                    .iter()
                    .find(|x| x.short_name == current)
                    .map_or(current.clone(), azure::Voice::label),
                _ => current.clone(),
            };
            let mut chosen = None;
            egui::ComboBox::from_id_salt("reader-tts-voice")
                .selected_text(shown)
                .width(250.0)
                .height(420.0)
                .show_ui(ui, |ui| match &self.voices {
                    Voices::Ready(v) => {
                        let mut group = "";
                        for x in v {
                            if x.locale_name != group {
                                group = &x.locale_name;
                                ui.label(RichText::new(group).small().weak());
                            }
                            if ui
                                .selectable_label(x.short_name == current, x.label())
                                .clicked()
                            {
                                chosen = Some(x.short_name.clone());
                            }
                        }
                    }
                    Voices::Loading(_) => {
                        ui.label("Loading voices…");
                    }
                    Voices::Failed(e) => {
                        ui.label(e);
                    }
                    Voices::None => {
                        ui.label("The voices show once the key and region are set.");
                    }
                });
            if let Some(v) = chosen {
                let slot = if self.traditional {
                    &mut s.voice_trad
                } else {
                    &mut s.voice
                };
                *slot = if v == azure::default_voice(self.traditional) {
                    String::new()
                } else {
                    v
                };
            }
            if ui
                .small_button("↻")
                .on_hover_text("Get the voice list again")
                .clicked()
            {
                self.fetch_voices(s, true);
            }
        });
    }

    fn key_field(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| match &self.key {
            KeyState::Unknown | KeyState::Loading(_) => {
                ui.label(RichText::new("Looking for the saved key…").weak());
            }
            KeyState::Loaded(Some(k)) if !self.key_editing => {
                let tail: String = k
                    .chars()
                    .rev()
                    .take(4)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                ui.label(format!("••••{tail}"))
                    .on_hover_text(format!("Saved, {}", key::where_kept()));
                if ui.small_button("Change").clicked() {
                    self.key_editing = true;
                    self.key_edit.clear();
                }
                if ui.small_button("Remove").clicked() {
                    self.save_key(String::new());
                }
            }
            KeyState::Loaded(_) => {
                ui.add(
                    egui::TextEdit::singleline(&mut self.key_edit)
                        .password(true)
                        .hint_text("Key 1 or Key 2")
                        .desired_width(200.0),
                );
                let k = self.key_edit.trim().to_owned();
                if ui
                    .add_enabled(
                        !k.is_empty() && self.key_saving.is_none(),
                        egui::Button::new("Save"),
                    )
                    .clicked()
                {
                    self.save_key(k);
                }
            }
        });
    }

    fn save_key(&mut self, k: String) {
        self.key_edit.clear();
        self.key_editing = false;
        self.key = KeyState::Loaded((!k.is_empty()).then(|| k.clone()));
        self.voices = Voices::None;
        let (tx, rx) = channel();
        let repaint = self.repaint.clone();
        let _ = std::thread::Builder::new()
            .name("tts-key".into())
            .spawn(move || {
                let _ = tx.send(key::store(&k));
                repaint();
            });
        self.key_saving = Some(rx);
        self.key_note = None;
    }

    /// Making audio ahead of time, and the saved audio. True: close the
    /// panel (the "Generate locally" window opens).
    fn ahead_of_time(
        &mut self,
        ui: &mut egui::Ui,
        s: &TtsSettings,
        spine: usize,
        ready: bool,
    ) -> bool {
        let mut close = false;
        let local = s.source == Source::Local;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Ahead of time").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("Generate locally…")
                    .on_hover_text(
                        "Choose chapters and make their audio with the voice on this computer",
                    )
                    .clicked()
                {
                    self.open_local_window(spine);
                    close = true;
                }
            });
        });
        ui.label(
            RichText::new(if local {
                "Make the audio now, to listen without waiting. \
                 Saved sentences are never made twice."
            } else {
                "Make the audio now, to listen without waiting or offline. \
                 Saved sentences are never paid for twice."
            })
            .size(12.0)
            .weak(),
        );
        let running = self
            .batch
            .as_ref()
            .is_some_and(|b| !b.finished() && !b.cancel.load(Ordering::Relaxed));
        let n_spines = self.chapters.lock().unwrap().len();
        // What, its sentences and billed characters, its chapters.
        type Row = (&'static str, Option<(usize, usize)>, Vec<usize>);
        let rows: [Row; 2] = [
            ("This chapter", self.estimate(spine, s.rate), vec![spine]),
            (
                "The whole book",
                self.book_estimate(s.rate),
                (0..n_spines).collect(),
            ),
        ];
        if !local {
            egui::Grid::new("reader-tts-ahead")
                .num_columns(3)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    for (what, est, spines) in rows {
                        ui.label(what);
                        match est {
                            Some((n, billed)) => ui.label(
                                RichText::new(format!(
                                    "{} sentences, ≤ {} characters billed",
                                    group(n),
                                    group(billed)
                                ))
                                .weak(),
                            ),
                            None => ui.label(RichText::new("counting…").weak()),
                        }
                        .on_hover_text(
                            "Azure counts each Chinese character twice; sentences \
                         already saved cost nothing",
                        );
                        if ui
                            .add_enabled(
                                ready && !running && est.is_some(),
                                egui::Button::new("Make"),
                            )
                            .clicked()
                        {
                            let w = what.to_lowercase();
                            self.pregenerate(s, spines, &w);
                        }
                        ui.end_row();
                    }
                });
        }
        self.batch_progress(ui);
        let sizes = self.cache_size;
        let mut delete = None;
        egui::Grid::new("reader-tts-saved")
            .num_columns(3)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                let (what, tip) = if local {
                    (
                        "This book's local audio",
                        "Delete this book's sentences made on this computer (Azure's stay)",
                    )
                } else {
                    (
                        "This book's Azure audio",
                        "Delete this book's sentences from Azure (those made on this computer stay)",
                    )
                };
                let every = if local {
                    "Delete every book's sentences made on this computer"
                } else {
                    "Delete every book's sentences from Azure"
                };
                for (all, what, tip) in [(false, what, tip), (true, "Every book's", every)] {
                    ui.label(what);
                    let u = sizes.map(|(b, a)| if all { a } else { b }.of(local));
                    match u {
                        Some(u) => ui.label(
                            RichText::new(format!(
                                "{} sentences, {:.1} MB",
                                group(u.files),
                                u.bytes as f64 / 1e6
                            ))
                            .weak(),
                        ),
                        None => ui.label(RichText::new("…").weak()),
                    };
                    if ui
                        .add_enabled(
                            !running && u.is_some_and(|u| u.files > 0),
                            egui::Button::new("Delete"),
                        )
                        .on_hover_text(tip)
                        .clicked()
                    {
                        delete = Some(all);
                    }
                    ui.end_row();
                }
            });
        if let Some(all) = delete {
            self.delete_cache(all, if local { OPUS } else { MP3 });
        }
        ui.label(
            RichText::new(format!("Kept in {}", self.dir.display()))
                .size(12.0)
                .weak(),
        );
        close
    }
}

impl Drop for Tts {
    fn drop(&mut self) {
        if let Some(e) = &self.engine {
            e.send(Cmd::Stop);
        }
    }
}

fn group(n: usize) -> String {
    super::group_digits(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(s: &str) -> Vec<String> {
        let c: Vec<char> = s.chars().collect();
        sentences(&c)
            .into_iter()
            .map(|r| c[r.start as usize..r.end as usize].iter().collect())
            .collect()
    }

    #[test]
    fn splits_at_sentence_ends_and_blocks() {
        assert_eq!(
            split("　　他说：“你好。”她笑了！\0\0第二段？是的……\0"),
            ["他说：“你好。”", "她笑了！", "第二段？", "是的……"]
        );
        // Nothing to read: dropped.
        assert_eq!(split("……\0　\0（）\0好"), ["好"]);
        assert!(split("").is_empty());
    }

    #[test]
    fn long_sentences_split_at_commas() {
        let long = "字".repeat(130) + "，" + &"字".repeat(10) + "。";
        let parts = split(&long);
        assert_eq!(parts.len(), 2);
        assert!(parts[0].ends_with('，'));
        let very = "字".repeat(700);
        assert_eq!(split(&very).len(), 3);
    }

    #[test]
    fn finds_the_sentence_of_a_character() {
        let c: Vec<char> = "一。　二。三。".chars().collect();
        let s = sentences(&c);
        assert_eq!(s, [0..2, 3..5, 5..7]);
        assert_eq!(sentence_at(&s, 0), 0);
        assert_eq!(sentence_at(&s, 1), 0);
        // The space between: the next sentence.
        assert_eq!(sentence_at(&s, 2), 1);
        assert_eq!(sentence_at(&s, 6), 2);
        assert_eq!(sentence_at(&s, 7), 3);
    }

    #[test]
    fn book_folders() {
        let a = book_dir("《酒与枪》番外集", "lib-1");
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("《酒与枪》番外集 ["), "{name}");
        assert_eq!(a.parent().unwrap(), cache_root());
        assert_ne!(a, book_dir("《酒与枪》番外集", "lib-2"));
        let b = book_dir("a/b:c?. ", "x");
        let name = b.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("a_b_c_ ["), "{name}");
        assert!(!book_dir("", "x").to_string_lossy().ends_with("] "));
        assert!(is_legacy("0d") && is_legacy("ff") && !is_legacy("0g") && !is_legacy("abc"));
    }

    #[test]
    fn keys_differ_by_voice_and_speed() {
        let a = Speech {
            voice: "zh-CN-XiaoxiaoNeural".into(),
            rate: 0,
            local: None,
        };
        let b = Speech {
            rate: 10,
            ..a.clone()
        };
        assert_ne!(a.key("你好"), b.key("你好"));
        assert_eq!(a.key("你好"), a.clone().key("你好"));
        assert_ne!(a.key("你好"), a.key("你好。"));
        // The local voice: its own keys and files, by every setting that
        // changes the audio, but not by how many requests run at once.
        let l = Speech {
            voice: String::new(),
            rate: 0,
            local: Some(local::LocalSettings::default()),
        };
        assert_ne!(l.key("你好"), a.key("你好"));
        assert_eq!((a.ext(), l.ext()), (MP3, OPUS));
        let with = |f: &dyn Fn(&mut local::LocalSettings)| {
            let mut s = local::LocalSettings::default();
            f(&mut s);
            Speech {
                local: Some(s),
                ..l.clone()
            }
            .key("你好")
        };
        assert_ne!(with(&|s| s.speaker = "serena".into()), l.key("你好"));
        assert_ne!(with(&|s| s.instruct = "慢一点".into()), l.key("你好"));
        assert_ne!(with(&|s| s.temperature = 0.7), l.key("你好"));
        assert_ne!(with(&|s| s.seed = 1), l.key("你好"));
        assert_ne!(with(&|s| s.chunk_chars = 120), l.key("你好"));
        assert_eq!(with(&|s| s.parallel = 1), l.key("你好"));
    }
}
