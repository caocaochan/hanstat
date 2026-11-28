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
pub(in crate::reader) mod audiobook;
pub mod azure;
pub mod duchinese;
mod engine;
pub mod key;
pub mod local;
mod local_ui;
pub mod opus;

use crate::reader::tree::Chapter;
use eframe::egui::{self, RichText};
pub use engine::State as PlayState;
use engine::{Availability, Batch, Chapters, Cmd, Engine, Now, Repaint, State};
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
    /// Azure's speed change in percent (part of the audio: changing it
    /// makes the sentences again).
    pub rate: i32,
    /// Playback speed (1.0: as made), for every voice: the sound is
    /// stretched as it is played, so no audio is made again.
    pub playback: f32,
    /// The dictionary popup pauses reading while it is open.
    pub pause_for_popup: bool,
    /// Play downloaded DuChinese native chapter audio instead of TTS when
    /// available for a DuChinese book.
    pub prefer_duchinese_audio: bool,
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
            playback: 1.0,
            pause_for_popup: true,
            prefer_duchinese_audio: true,
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

    fn speech(&self, source: Source, traditional: bool) -> Speech {
        match source {
            Source::Azure => Speech {
                voice: self.voice(traditional),
                rate: self.rate,
                local: None,
            },
            Source::Local => Speech {
                voice: String::new(),
                rate: 0,
                local: Some(self.local.clone()),
            },
        }
    }

    pub(super) fn use_speech(&mut self, speech: &Speech, traditional: bool) {
        if let Some(local) = &speech.local {
            self.source = Source::Local;
            self.local = local.clone();
        } else {
            self.source = Source::Azure;
            self.rate = speech.rate;
            if traditional {
                self.voice_trad = speech.voice.clone();
            } else {
                self.voice = speech.voice.clone();
            }
        }
    }
}

/// How sentences are read: a clip made with one is not used for another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Speech {
    pub voice: String,
    pub rate: i32,
    /// The local voice and its settings (`voice` and `rate` unused); `None`: Azure.
    pub local: Option<local::LocalSettings>,
}

impl Speech {
    /// The prefix hashed before every sentence's text in [`Self::key`].
    pub(super) fn key_prefix(&self) -> String {
        match &self.local {
            None => format!(
                "{}\u{1}{}\u{1}{}\u{1}",
                self.voice,
                self.rate,
                azure::FORMAT
            ),
            Some(l) => format!("{}\u{1}", l.key_text()),
        }
    }

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

    /// Save settings separately from the clips, whose names are one-way hashes.
    fn record(&self, dir: &Path) -> std::io::Result<()> {
        let profiles = dir.join("profiles");
        let path = profiles.join(format!("{:016x}.json", self.key("")));
        if path.is_file() {
            return Ok(());
        }
        std::fs::create_dir_all(&profiles)?;
        let mut file = tempfile::NamedTempFile::new_in(profiles)?;
        serde_json::to_writer(&mut file, self)?;
        persist_clip(file, &path)
    }

    fn decode(&self, bytes: Vec<u8>) -> Result<(Vec<f32>, u32), String> {
        if self.local.is_some() {
            opus::decode(bytes)
        } else {
            audio::decode_mp3(bytes)
        }
    }

    /// Decode a saved clip into `out` (reusing its allocation). For Opus clips,
    /// `bytes` is borrowed in place so its buffer can be reused too.
    pub(super) fn decode_into(
        &self,
        bytes: &mut Vec<u8>,
        out: &mut Vec<f32>,
    ) -> Result<u32, String> {
        if self.local.is_some() {
            opus::decode_into(bytes, out)
        } else {
            audio::decode_mp3_into(std::mem::take(bytes), out)
        }
    }
}

/// An Azure region and key.
type AzureClient = (String, String);

/// Audio for Anki cards, made the way read aloud reads: saved with the
/// book's read-aloud clips (a sentence already heard costs nothing).
pub struct CardVoice {
    speech: Speech,
    azure: Option<Arc<azure::Azure>>,
    dir: PathBuf,
}

impl CardVoice {
    /// The saved clip of `text` (made now if there is none; slow: a
    /// request to Azure, or the local voice, which may have to load).
    pub fn clip(&self, text: &str) -> Result<PathBuf, String> {
        if text.trim().is_empty() {
            return Err("Nothing to say".into());
        }
        let (key, ext) = (self.speech.key(text), self.speech.ext());
        if let Some(p) = saved_clip(&self.dir, key, ext) {
            return Ok(p);
        }
        let bytes = match &self.speech.local {
            Some(l) => opus::encode(&local::preview(text, l)?)?,
            None => self
                .azure
                .as_ref()
                .ok_or("Add your Azure Speech key and region for audio")?
                .synth(text, &self.speech.voice, self.speech.rate)
                .map_err(|e| e.to_string())?,
        };
        let path = clip_path(&self.dir, key, ext);
        let save = || -> std::io::Result<()> {
            use std::io::Write;
            std::fs::create_dir_all(&self.dir)?;
            let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
            file.write_all(&bytes)?;
            persist_clip(file, &path)
        };
        save().map_err(|e| format!("The audio can't be saved: {e}"))?;
        let _ = self.speech.record(&self.dir);
        Ok(path)
    }

    /// The file name a clip of `text` gets in Anki's media folder: the
    /// same text in the same voice, the same file.
    pub fn media_name(&self, text: &str) -> String {
        format!(
            "hanstat_{:016x}.{}",
            self.speech.key(text),
            self.speech.ext()
        )
    }
}

fn persist_clip(file: tempfile::NamedTempFile, path: &Path) -> std::io::Result<()> {
    match file.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists && path.is_file() => Ok(()),
        Err(e) => Err(e.error),
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
    if (std::fs::create_dir_all(dir).is_ok() && std::fs::rename(&old, &p).is_ok()) || p.exists() {
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

/// The spoken sentences of a chapter: [`sentences`] with DuChinese sentence
/// translations ([`Chapter::spoiler_ranges`]) excluded, whether blurred or
/// revealed.
pub fn sentences_of(ch: &Chapter) -> Vec<Range<u32>> {
    sentences_excluding(&ch.chars, &ch.spoiler_ranges)
}

pub(super) fn sentences_excluding(chars: &[char], skip: &[Range<u32>]) -> Vec<Range<u32>> {
    let mut out = sentences(chars);
    if !skip.is_empty() {
        out.retain(|r| !skip.iter().any(|s| r.start < s.end && s.start < r.end));
    }
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
    sent_availability: Availability,
    sent_prefer_du: Option<bool>,
    /// Native DuChinese chapter audio resolver (for DuChinese EPUBs).
    du: Option<Arc<duchinese::DuBook>>,
    /// Cached `(chapters_with_audio, total_chapters)` for `du`.
    du_coverage: Option<(usize, usize)>,
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
    /// First source character after the page or spread shown.
    pub page_end: u32,
    /// The local voice's panel and window.
    local: local_ui::State,
    /// The chapter shown (the preview reads from it).
    local_spine: usize,
    /// Sentences and billed characters: per chapter, and the book's.
    estimates: std::collections::HashMap<(usize, i32), (usize, usize)>,
    book_estimate: Option<(i32, usize, usize)>,
    book_estimate_job: Option<(i32, Receiver<(usize, usize)>)>,
    /// Whether Azure's voice-speed slider is currently being dragged.
    rate_dragging: bool,
    /// The sentence heard last frame and whether it was on the pages shown
    /// (the pages follow it only then).
    pub last_at: Option<(usize, u32)>,
    pub last_visible: bool,
    /// The playback speed the engine was last told, and whether its slider
    /// is being dragged (it is told once let go).
    sent_playback: Option<f32>,
    playback_dragging: bool,
    /// Anki cards' audio, made as read aloud reads now (kept while that
    /// stays the same).
    card_voice: Option<(Speech, Option<AzureClient>, Arc<CardVoice>)>,
    /// The dictionary popup was open last frame, and paused the reading.
    popup_open: bool,
    paused_by_popup: bool,
    /// Paused for a sound over it (the daily goal met) until then
    /// ([`crate::reading_stats::now`]), and resumed after.
    paused_until: Option<f64>,
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
            sent_availability: Availability::Waiting,
            sent_prefer_du: None,
            du: None,
            du_coverage: None,
            batch: None,
            cache_size: None,
            cache_job: None,
            labels,
            page_src: 0,
            page_end: u32::MAX,
            local: local_ui::State::default(),
            local_spine: 0,
            estimates: Default::default(),
            book_estimate: None,
            book_estimate_job: None,
            rate_dragging: false,
            last_at: None,
            last_visible: false,
            sent_playback: None,
            playback_dragging: false,
            card_voice: None,
            popup_open: false,
            paused_by_popup: false,
            paused_until: None,
        }
    }

    /// Attach a DuChinese native-audio resolver for this book.
    pub fn with_duchinese(mut self, du: Option<duchinese::DuBook>) -> Self {
        self.du_coverage = du.as_ref().map(duchinese::DuBook::coverage);
        self.du = du.map(Arc::new);
        self
    }

    fn du_active_at(&self, spine: usize, s: &TtsSettings) -> bool {
        s.prefer_duchinese_audio && self.du.as_ref().is_some_and(|d| d.chapter_has_audio(spine))
    }

    fn speech(&self, s: &TtsSettings) -> Speech {
        s.speech(s.source, self.traditional)
    }

    fn the_key(&self) -> Option<&str> {
        match &self.key {
            KeyState::Loaded(Some(k)) => Some(k),
            _ => None,
        }
    }

    /// Look for the Azure key now (Anki cards may need it before the
    /// panel is ever opened).
    pub fn prepare_key(&mut self) {
        self.load_key();
    }

    /// Anki cards' word and sentence audio, in the voice read aloud uses;
    /// `None` while there is none ready (no key, or no local model).
    pub fn card_voice(&mut self, s: &TtsSettings) -> Option<Arc<CardVoice>> {
        if !self.client_ready(s) {
            return None;
        }
        let speech = self.speech(s);
        let client = match s.source {
            Source::Azure => Some((s.region.trim().to_owned(), self.the_key()?.to_owned())),
            Source::Local => None,
        };
        if let Some((sp, c, v)) = &self.card_voice
            && *sp == speech
            && *c == client
        {
            return Some(v.clone());
        }
        let v = Arc::new(CardVoice {
            speech: speech.clone(),
            azure: client
                .as_ref()
                .map(|(r, k)| Arc::new(azure::Azure::new(r, k))),
            dir: self.dir.clone(),
        });
        self.card_voice = Some((speech, client, v.clone()));
        Some(v)
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
                self.du.clone(),
                s.prefer_duchinese_audio,
                self.repaint.clone(),
            ));
            self.sent_speech = Some(self.speech(s));
            self.sent_client = None;
            self.sent_availability = Availability::Waiting;
            self.sent_prefer_du = Some(s.prefer_duchinese_audio);
            let e = self.engine.as_ref().unwrap();
            e.send(Cmd::Playback(playback_speed(s.playback)));
            self.sent_playback = Some(s.playback);
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
        if let Some((rate, rx)) = &self.book_estimate_job
            && let Ok((sent, billed)) = rx.try_recv()
        {
            self.book_estimate = Some((*rate, sent, billed));
            self.book_estimate_job = None;
        }
        let Some(engine) = &self.engine else { return };
        let now = engine.now();
        if now.needs_setup && !self.now.needs_setup {
            self.panel_open = true;
        }
        self.now = now;
        let speech = self.speech(s);
        if self.sent_speech.as_ref() != Some(&speech) {
            engine.send(Cmd::Speech(speech.clone()));
            self.sent_speech = Some(speech);
        }
        if self.sent_prefer_du != Some(s.prefer_duchinese_audio) {
            engine.send(Cmd::PreferDu(s.prefer_duchinese_audio));
            self.sent_prefer_du = Some(s.prefer_duchinese_audio);
        }
        if !self.playback_dragging && self.sent_playback != Some(s.playback) {
            engine.send(Cmd::Playback(playback_speed(s.playback)));
            self.sent_playback = Some(s.playback);
        }
        let client = self
            .the_key()
            .filter(|_| azure::valid_region(s.region.trim()))
            .map(|k| (s.region.trim().to_owned(), k.to_owned()));
        if client != self.sent_client {
            engine.send(Cmd::Client(
                client
                    .as_ref()
                    .map(|(r, k)| Arc::new(azure::Azure::new(r, k))),
            ));
            self.sent_client = client;
        }
        let availability = if self.client_ready(s) {
            Availability::Ready
        } else {
            match s.source {
                Source::Azure if matches!(self.key, KeyState::Loading(_) | KeyState::Unknown) => {
                    Availability::Waiting
                }
                Source::Azure => Availability::Setup(
                    "Add your Azure Speech key and region to generate the next sentence.".into(),
                ),
                Source::Local if self.design_making() => Availability::Waiting,
                Source::Local if s.local.designed.is_some() => {
                    Availability::Setup(local::DESIGN_NOT_DOWNLOADED.into())
                }
                Source::Local => Availability::Setup(local::NOT_DOWNLOADED.into()),
            }
        };
        if self.sent_availability != availability {
            engine.send(Cmd::Availability(availability.clone()));
            self.sent_availability = availability;
        }
    }

    fn client_ready(&self, s: &TtsSettings) -> bool {
        match s.source {
            Source::Azure => self.the_key().is_some() && azure::valid_region(s.region.trim()),
            Source::Local => self.local.downloaded(&s.local) && !self.design_making(),
        }
    }

    pub fn state(&self) -> State {
        self.now.state
    }

    #[cfg(test)]
    pub fn error(&self) -> Option<&str> {
        self.now.error.as_deref()
    }

    /// Reading aloud (or paused, or waiting for audio).
    pub fn active(&self) -> bool {
        self.state() != State::Stopped
    }

    /// The sentence heard: its chapter and characters.
    pub fn at(&self) -> Option<(usize, Range<u32>)> {
        self.now.at.clone()
    }

    /// While the sentence is heard: when its voice started and how long.
    pub fn clock(&self) -> Option<engine::Clock> {
        self.now.clock
    }

    /// Read from the sentence holding character `src` of chapter `spine`.
    pub fn play_from(&mut self, spine: usize, src: u32, s: &TtsSettings) {
        self.paused_by_popup = false;
        self.paused_until = None;
        if s.source == Source::Azure && !self.du_active_at(spine, s) {
            self.load_key();
        } else if s.source != Source::Azure {
            self.stop_preview();
        }
        self.engine(s);
        self.last_visible = true;
        self.last_at = None;
        self.poll(s);
        if let Some(e) = &self.engine {
            e.send(Cmd::Play { spine, src });
        }
        self.now.state = State::Loading;
        self.now.needs_setup = false;
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

    /// Pause the reading until `until` ([`crate::reading_stats::now`]),
    /// for a sound to be heard over it; [`Self::sound_pause`] resumes it.
    pub fn pause_until(&mut self, until: f64) {
        if self.paused_until.is_some() {
            self.paused_until = Some(until);
        } else if matches!(self.now.state, State::Loading | State::Playing)
            && let Some(e) = &self.engine
        {
            e.send(Cmd::Pause);
            self.now.state = State::Paused;
            self.paused_until = Some(until);
        }
    }

    /// Once a frame: resume a reading paused by [`Self::pause_until`] once
    /// its time is up; if the popup, which pauses it (if set), is open
    /// then, it stays paused until the popup closes. The seconds left, to
    /// wake for.
    pub fn sound_pause(&mut self, now: f64, s: &TtsSettings) -> Option<f64> {
        let until = self.paused_until?;
        if self.now.state == State::Stopped {
            self.paused_until = None;
            return None;
        }
        if now < until {
            return Some(until - now);
        }
        self.paused_until = None;
        if self.popup_open && s.pause_for_popup {
            self.paused_by_popup = true;
        } else if let Some(e) = &self.engine {
            e.send(Cmd::Resume);
            self.now.state = State::Loading;
        }
        None
    }

    pub fn toggle_pause(&mut self) {
        // Paused or resumed by hand: the popup and a sound leave it so.
        self.paused_by_popup = false;
        self.paused_until = None;
        if let Some(e) = &self.engine {
            match self.now.state {
                State::Paused => e.send(Cmd::Resume),
                State::Loading | State::Playing => e.send(Cmd::Pause),
                State::Stopped => {}
            }
        }
    }

    /// Back (negative) or forward by `by` sentences while reading.
    pub fn step(&mut self, by: i32) {
        if !self.active() {
            return;
        }
        if let Some(e) = &self.engine {
            e.send(Cmd::Step(by));
        }
    }

    pub fn stop(&mut self) {
        self.paused_by_popup = false;
        self.paused_until = None;
        if let Some(e) = &self.engine {
            e.send(Cmd::Stop);
        }
        self.now.state = State::Stopped;
        self.now.at = None;
        self.now.needs_setup = false;
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
        let s = sentences_of(&ch);
        let mut buf = String::new();
        let mut billed = 0usize;
        for r in &s {
            buf.clear();
            buf.extend(&ch.chars[r.start as usize..r.end as usize]);
            billed += azure::billed(&buf, rate);
        }
        let e = (s.len(), billed);
        self.estimates.insert((spine, rate), e);
        Some(e)
    }

    /// The book's, computed asynchronously once every chapter is built.
    fn book_estimate(&mut self, rate: i32) -> Option<(usize, usize)> {
        if let Some((r, n, b)) = self.book_estimate
            && r == rate
        {
            return Some((n, b));
        }
        if self.rate_dragging {
            return None;
        }
        if self
            .book_estimate_job
            .as_ref()
            .is_some_and(|(r, _)| *r == rate)
        {
            return None;
        }
        let chapters: Vec<Arc<Chapter>> = {
            let guard = self.chapters.lock().unwrap();
            if guard.iter().any(Option::is_none) {
                return None;
            }
            guard.iter().filter_map(Clone::clone).collect()
        };
        let (tx, rx) = channel();
        let repaint = self.repaint.clone();
        let _ = std::thread::Builder::new()
            .name("tts-book-estimate".into())
            .spawn(move || {
                let (mut sent, mut billed) = (0usize, 0usize);
                let mut buf = String::new();
                for ch in &chapters {
                    let s = sentences_of(ch);
                    sent += s.len();
                    for r in &s {
                        buf.clear();
                        buf.extend(&ch.chars[r.start as usize..r.end as usize]);
                        billed += azure::billed(&buf, rate);
                    }
                }
                let _ = tx.send((sent, billed));
                repaint();
            });
        self.book_estimate_job = Some((rate, rx));
        None
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
    pub(super) fn set_test_cache_dir(&mut self, dir: PathBuf) {
        self.dir = dir;
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

    /// The controls in the reader's bar: the panel's button and, while
    /// reading, the transport icons and the playback speed.
    pub fn bar(&mut self, ui: &mut egui::Ui, s: &mut TtsSettings) -> egui::Response {
        self.local.apply_pending_designed(&mut s.local);
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
            self.transport(ui, 14.0);
            self.playback_slider(ui, s, 90.0);
        }
        b
    }

    /// ⏮ ⏸/▶ ⏹ ⏭: previous sentence, pause or resume, stop, next sentence.
    fn transport(&mut self, ui: &mut egui::Ui, size: f32) {
        let paused = self.state() == State::Paused;
        ui.spacing_mut().item_spacing.x = 2.0;
        if icon_button(
            ui,
            Icon::Prev,
            size,
            "Previous sentence (Ctrl+←)",
            "Previous sentence",
        )
        .clicked()
        {
            self.step(-1);
        }
        let (icon, tip, name) = if paused {
            (Icon::Play, "Resume (K)", "Resume")
        } else {
            (Icon::Pause, "Pause (K)", "Pause")
        };
        if icon_button(ui, icon, size, tip, name).clicked() {
            self.toggle_pause();
        }
        if icon_button(ui, Icon::Stop, size, "Stop reading aloud", "Stop").clicked() {
            self.stop();
        }
        if icon_button(
            ui,
            Icon::Next,
            size,
            "Next sentence (Ctrl+→)",
            "Next sentence",
        )
        .clicked()
        {
            self.step(1);
        }
        ui.spacing_mut().item_spacing.x = 8.0;
    }

    /// The playback speed, 0.5× to 2×: told to the engine once let go.
    fn playback_slider(&mut self, ui: &mut egui::Ui, s: &mut TtsSettings, width: f32) {
        ui.spacing_mut().slider_width = width;
        let r = ui
            .add(
                egui::Slider::new(&mut s.playback, PLAYBACK_MIN..=PLAYBACK_MAX)
                    .step_by(0.05)
                    .show_value(true)
                    .custom_formatter(|v, _| format!("{v:.2}×"))
                    .custom_parser(|t| t.trim().trim_end_matches(['×', 'x']).trim().parse().ok()),
            )
            .on_hover_text("Playback speed, for every voice: no audio is made again. Double-click for normal speed");
        if r.double_clicked() {
            s.playback = 1.0;
        }
        self.playback_dragging = r.dragged();
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
        self.local.apply_pending_designed(&mut s.local);
        self.rate_dragging = false;
        let ctx = button.ctx.clone();
        let mut open = self.panel_open;
        let design_was_open = self.design_window_open();
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
                let max = crate::palette::dropdown_max_size(ui.ctx(), 72.0);
                crate::palette::resizable_scroll_popup(
                    ui,
                    "reader-tts-box",
                    egui::vec2(420.0, (max.y * 0.85).min(560.0)),
                    egui::vec2(260.0, 160.0),
                    max,
                    |ui| {
                // Playback.
                ui.horizontal(|ui| {
                    match self.state() {
                        State::Stopped => {
                            if ui.button("Read from this page").clicked() {
                                action = Some(Action::PlayPage);
                            }
                        }
                        st => {
                            self.transport(ui, 16.0);
                            ui.label(
                                RichText::new(match st {
                                    State::Loading if self.now.using_duchinese => {
                                        "Loading DuChinese audio…"
                                    }
                                    State::Loading if s.source == Source::Local => {
                                        if local::loaded(s.local.mode(), s.local.parallel) {
                                            "Making the audio…"
                                        } else {
                                            "Loading the voice…"
                                        }
                                    }
                                    State::Loading => "Waiting for Azure…",
                                    State::Paused => "Paused",
                                    _ if self.now.using_duchinese => "DuChinese audio",
                                    _ => "Reading",
                                })
                                .weak(),
                            );
                        }
                    }
                });
                ui.label(
                    RichText::new("Right-click text and choose Play from here. While reading, you can also click a word to jump to its sentence.")
                        .size(12.0)
                        .weak(),
                );
                if let Some(e) = &self.now.error {
                    ui.colored_label(crate::reader::theme::get(ui.ctx()).red, e);
                }
                if let Some((have, total)) = self.du_coverage
                    && have > 0
                {
                    ui.separator();
                    let label = if have >= total {
                        "Use DuChinese audio".to_owned()
                    } else {
                        format!("Use DuChinese audio ({have} of {total} chapters)")
                    };
                    ui.checkbox(&mut s.prefer_duchinese_audio, label)
                        .on_hover_text(
                            "Play DuChinese's downloaded native narration synced to the text \
                             instead of TTS. Chapters without downloaded audio fall back to TTS.",
                        );
                    if self.du_active_at(spine, s) {
                        ui.label(
                            RichText::new(
                                "DuChinese's native narration is active for this chapter. \
                                 Uncheck above to use Azure or local TTS instead.",
                            )
                            .size(12.0)
                            .weak(),
                        );
                    }
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
                        ui.label("Voice speed");
                        let rate_resp = ui.add(
                            egui::Slider::new(&mut s.rate, -50..=100)
                                .step_by(5.0)
                                .custom_formatter(|v, _| {
                                    if v == 0.0 { "normal".to_owned() } else { format!("{v:+.0} %") }
                                }),
                        )
                        .on_hover_text("Azure reads faster or slower: makes the audio again. For a speed that needs no new audio, use Playback speed");
                        self.rate_dragging = rate_resp.dragged();
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
                        crate::reader::theme::get(ui.ctx()).red,
                        "A region is a name such as eastus or westeurope.",
                    );
                }
                if let Some((ok, note)) = &self.key_note {
                    let c = if *ok { ui.visuals().weak_text_color() } else { crate::reader::theme::get(ui.ctx()).red };
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
                    },
                );
            });
        if close || (!design_was_open && self.design_window_open()) {
            open = false;
        }
        self.panel_open = open;
        self.local_window(&ctx, s, spine);
        self.design_window(&ctx, s, spine);
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
                ui.colored_label(crate::reader::theme::get(ui.ctx()).red, e);
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
                .small_button(crate::icons::glyph(
                    ui.ctx(),
                    crate::icons::ARROW_CLOCKWISE,
                    false,
                ))
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
    /// panel (a local voice window opens).
    fn ahead_of_time(
        &mut self,
        ui: &mut egui::Ui,
        s: &TtsSettings,
        spine: usize,
        ready: bool,
    ) -> bool {
        let mut close = false;
        let local = s.source == Source::Local;
        ui.horizontal_top(|ui| {
            ui.label(RichText::new("Ahead of time").strong());
            ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
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
                if local
                    && ui
                        .add_enabled(
                            !self.local_voice_busy(),
                            egui::Button::new("Design a voice…"),
                        )
                        .on_hover_text(
                            "Describe a voice, hear a sample and keep it to read your books",
                        )
                        .clicked()
                {
                    self.open_design_window();
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
        if !local {
            // What, its sentences and billed characters, whether it's the whole book.
            type Row = (&'static str, Option<(usize, usize)>, bool);
            let rows: [Row; 2] = [
                ("This chapter", self.estimate(spine, s.rate), false),
                ("The whole book", self.book_estimate(s.rate), true),
            ];
            let mut make: Option<(&'static str, bool)> = None;
            egui::Grid::new("reader-tts-ahead")
                .num_columns(3)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    for (what, est, whole_book) in rows {
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
                            make = Some((what, whole_book));
                        }
                        ui.end_row();
                    }
                });
            if let Some((what, whole_book)) = make {
                let spines = if whole_book {
                    let n_spines = self.chapters.lock().unwrap().len();
                    (0..n_spines).collect()
                } else {
                    vec![spine]
                };
                let w = what.to_lowercase();
                self.pregenerate(s, spines, &w);
            }
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

/// The slowest and fastest playback speeds.
pub const PLAYBACK_MIN: f32 = 0.5;
pub const PLAYBACK_MAX: f32 = 2.0;

/// A saved playback speed made safe (an edited settings file).
pub fn playback_speed(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(PLAYBACK_MIN, PLAYBACK_MAX)
    } else {
        1.0
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Icon {
    Prev,
    Play,
    Pause,
    Stop,
    Next,
}

/// A transport icon (Phosphor's filled glyph) in the text colour, on the
/// button's background when hovered. `name` is what screen
/// readers (and UI Automation) call it.
fn icon_button(ui: &mut egui::Ui, icon: Icon, size: f32, tip: &str, name: &str) -> egui::Response {
    let pad = ui.spacing().button_padding.y;
    let side = size + 2.0 * pad + 4.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
    let name = name.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
    if ui.is_rect_visible(rect) {
        let v = ui.style().interact(&resp);
        let p = ui.painter();
        if resp.hovered() || resp.has_focus() {
            p.rect_filled(rect, v.corner_radius, v.weak_bg_fill);
        }
        crate::reader::icons::paint(
            p,
            rect.center(),
            icon.glyph(),
            size * 1.15,
            true,
            v.fg_stroke.color,
        );
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(tip)
}

impl Icon {
    fn glyph(self) -> char {
        use crate::reader::icons;
        match self {
            Icon::Prev => icons::SKIP_BACK,
            Icon::Play => icons::PLAY,
            Icon::Pause => icons::PAUSE,
            Icon::Stop => icons::STOP,
            Icon::Next => icons::SKIP_FORWARD,
        }
    }
}

fn group(n: usize) -> String {
    super::group_digits(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_icons_and_speed_are_drawn() {
        // The bar while reading: Phosphor's glyphs, not missing-glyph boxes,
        // and the speed slider. Pictures in TTS_LAYOUT_OUT, if set.
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut tts = Tts::new(
            Arc::new(Mutex::new(Vec::new())),
            Vec::new(),
            false,
            "",
            "layout-test",
            &ctx,
        );
        let mut settings = TtsSettings {
            playback: 1.25,
            ..Default::default()
        };
        let mut screen = crate::raster::Screen::new();
        let size = egui::vec2(420.0, 60.0);
        let mut glyphs = Vec::new();
        for i in 0..3 {
            let mut frame = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    egui::Panel::top("bar").show(ui, |ui| {
                        ui.horizontal(|ui| {
                            tts.transport(ui, 14.0);
                            tts.playback_slider(ui, &mut settings, 90.0);
                        });
                    });
                },
            );
            screen.absorb(&mut frame);
            if i == 2 {
                fn walk(s: &egui::Shape, out: &mut Vec<char>) {
                    match s {
                        egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                        egui::Shape::Text(t) => out.extend(t.galley.text().chars()),
                        _ => {}
                    }
                }
                frame
                    .shapes
                    .iter()
                    .for_each(|s| walk(&s.shape, &mut glyphs));
                if let Some(dir) = std::env::var_os("TTS_LAYOUT_OUT") {
                    let dir = PathBuf::from(dir);
                    std::fs::create_dir_all(&dir).unwrap();
                    screen
                        .picture(&ctx, &mut frame, size)
                        .write_png(&dir.join("transport.png"))
                        .unwrap();
                }
            }
        }
        use crate::reader::icons;
        for g in [
            icons::SKIP_BACK,
            icons::PAUSE,
            icons::STOP,
            icons::SKIP_FORWARD,
        ] {
            assert!(glyphs.contains(&g), "{:x} drawn", g as u32);
        }
        let shown: String = glyphs.iter().collect();
        assert!(shown.contains("1.25×"), "{shown}");
    }

    #[test]
    fn local_action_buttons_are_stacked_without_extra_space() {
        fn text_rect(shape: &egui::Shape, label: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| text_rect(s, label)),
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            }
        }

        let ctx = egui::Context::default();
        let mut tts = Tts::new(
            Arc::new(Mutex::new(Vec::new())),
            Vec::new(),
            false,
            "",
            "layout-test",
            &ctx,
        );
        let settings = TtsSettings {
            source: Source::Local,
            ..Default::default()
        };
        let mut screen = crate::raster::Screen::new();
        let size = egui::vec2(460.0, 600.0);
        for _ in 0..30 {
            let mut frame = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    tts.ahead_of_time(ui, &settings, 0, false);
                },
            );
            screen.absorb(&mut frame);
            let rect = |label| {
                frame
                    .shapes
                    .iter()
                    .find_map(|s| text_rect(&s.shape, label))
                    .unwrap()
            };
            let generate = rect("Generate locally…");
            let design = rect("Design a voice…");
            assert!(
                (0.0..=12.0).contains(&(design.top() - generate.bottom())),
                "{generate:?}, {design:?}"
            );
            assert!(
                (design.right() - generate.right()).abs() < 1.0,
                "right-aligned buttons"
            );
            if let Some(dir) = std::env::var_os("TTS_LAYOUT_OUT") {
                let dir = PathBuf::from(dir);
                std::fs::create_dir_all(&dir).unwrap();
                screen
                    .picture(&ctx, &mut frame, size)
                    .write_png(&dir.join("local-actions.png"))
                    .unwrap();
            }
        }
    }

    /// A sound's pause: resumed when its time is up; not after a pause or
    /// resume by hand, or a stop; kept for the popup if it is open then.
    /// (The engine is idle: the states are what the reader tells it.)
    #[test]
    fn sound_pause_resumes_after_and_yields_to_the_hand_and_the_popup() {
        let ctx = egui::Context::default();
        let s = TtsSettings::default();
        let mut tts = Tts::new(
            Arc::new(Mutex::new(Vec::new())),
            Vec::new(),
            false,
            "",
            "pause-test",
            &ctx,
        );
        // Not reading aloud: nothing to pause.
        tts.pause_until(10.0);
        assert_eq!(tts.paused_until, None);
        tts.engine(&s);

        tts.now.state = State::Playing;
        tts.pause_until(10.0);
        assert_eq!(tts.state(), State::Paused);
        assert_eq!(tts.sound_pause(6.0, &s), Some(4.0));
        assert_eq!(tts.state(), State::Paused);
        assert_eq!(tts.sound_pause(10.0, &s), None);
        assert_eq!(tts.state(), State::Loading, "resumed");
        assert_eq!(tts.paused_until, None);

        // Paused (or resumed) by hand meanwhile: left so.
        tts.now.state = State::Playing;
        tts.pause_until(20.0);
        tts.toggle_pause();
        assert_eq!(tts.sound_pause(25.0, &s), None);
        assert_eq!(tts.state(), State::Paused, "not resumed");

        // The popup open when the time is up: it resumes once it closes.
        tts.now.state = State::Playing;
        tts.popup(false, &s);
        tts.pause_until(30.0);
        tts.popup(true, &s);
        assert_eq!(tts.sound_pause(31.0, &s), None);
        assert_eq!(tts.state(), State::Paused, "kept for the popup");
        tts.popup(false, &s);
        assert_eq!(tts.state(), State::Loading, "resumed with the popup closed");

        // Stopped meanwhile: forgotten.
        tts.now.state = State::Playing;
        tts.pause_until(40.0);
        tts.stop();
        assert_eq!(tts.paused_until, None);
    }

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

    #[test]
    fn duchinese_translations_are_excluded_from_tts() {
        use super::super::css::RuleSet;
        use super::super::tree::{Source as TreeSource, build_doc, ua_rules};

        let html = r#"<html><body>
            <h2>小猫</h2>
            <p>这是一只猫。它很可爱。</p>
            <p class="tr" lang="en">This is a cat. It is cute.</p>
            <p>它喜欢吃鱼。</p>
            <p class="tr" lang="en">It likes to eat fish.</p>
        </body></html>"#;
        let doc = scraper::Html::parse_document(html);
        let ch = build_doc(
            &doc,
            &ua_rules(),
            &RuleSet::default(),
            TreeSource::Epub {
                base: String::new(),
                path: String::new(),
                duchinese: true,
            },
        );
        let spoken: Vec<String> = sentences_of(&ch)
            .into_iter()
            .map(|r| ch.chars[r.start as usize..r.end as usize].iter().collect())
            .collect();
        assert_eq!(
            spoken,
            ["小猫", "这是一只猫。", "它很可爱。", "它喜欢吃鱼。"]
        );

        let audio = engine::chapter_sentences(&ch.chars, &ch.spoiler_ranges);
        let audio_texts: Vec<&str> = audio.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(
            audio_texts,
            ["小猫", "这是一只猫。", "它很可爱。", "它喜欢吃鱼。"]
        );
    }
}
