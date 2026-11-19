//! Anki cards from the popup: each entry's **+** adds a note for its word
//! through AnkiConnect ([`hanstat::anki_connect`]), filled from the entry,
//! the book's sentence and the book's title by templates per field
//! (Yomitan's markers, such as `{expression}` and `{sentence}`). The button is
//! greyed out when the word is already in Anki.
//!
//! Nothing here runs on the UI thread but channel sends and a map lookup: a
//! worker thread builds the notes and talks to Anki. When a popup opens, its
//! entries are checked there (duplicates) while the popup shows at once; the
//! buttons follow a frame or so later.

use hanstat::anki_connect::{self, CanAdd, Client, Note};
use hanstat::dict::lookup::{DictInfo, Group, Lookup};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// How cards are made: kept in the reader's settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CardSettings {
    /// Show the **+** button in the popup.
    pub enabled: bool,
    /// AnkiConnect's address and API key (empty: none).
    pub url: String,
    pub key: String,
    pub deck: String,
    /// The note type.
    pub model: String,
    /// Its fields in order, each with its template.
    pub fields: Vec<(String, String)>,
    /// Space-separated.
    pub tags: String,
    /// A duplicate is a note of any type with the same first field, not only
    /// of this type.
    pub check_all_models: bool,
}

impl Default for CardSettings {
    fn default() -> Self {
        CardSettings {
            enabled: true,
            url: anki_connect::DEFAULT_URL.to_owned(),
            key: String::new(),
            deck: String::new(),
            model: String::new(),
            fields: Vec::new(),
            tags: "hanstat".to_owned(),
            check_all_models: false,
        }
    }
}

impl CardSettings {
    /// A deck and note type are chosen, and the first field has a template.
    pub fn ready(&self) -> bool {
        !self.deck.is_empty()
            && !self.model.is_empty()
            && self
                .fields
                .first()
                .is_some_and(|(_, t)| !t.trim().is_empty())
    }
}

/// The markers a field's template can hold, with what they become.
pub const MARKERS: &[(&str, &str)] = &[
    ("{expression}", "The word, as the dictionary writes it"),
    ("{reading}", "Its reading (pinyin)"),
    ("{main-definition}", "The first dictionary's definitions"),
    ("{glossary}", "Every dictionary's definitions"),
    (
        "{glossary-first}",
        "The first dictionary's definitions (as {main-definition})",
    ),
    ("{sentence}", "The book's sentence, the word in bold"),
    ("{cloze-prefix}", "The sentence before the word"),
    ("{cloze-body}", "The word as the book writes it"),
    ("{cloze-suffix}", "The sentence after the word"),
    ("{document-title}", "The book's title"),
    ("{frequencies}", "The frequency lists' ranks, as a list"),
    (
        "{frequency-harmonic-rank}",
        "The harmonic mean of the frequency lists' ranks (9999999 with none), to sort by",
    ),
    (
        "{audio}",
        "Empty: hanstat has no audio (HyperTTS can add it in Anki)",
    ),
    ("{screenshot}", "Empty: hanstat takes no screenshots"),
    (
        "{popup-selection-text}",
        "Empty: hanstat has no text selection",
    ),
];

/// The default templates (Yomitan's, as set up for Lapis), by field name.
const DEFAULTS: &[(&str, &str)] = &[
    ("Expression", "{expression}"),
    ("ExpressionReading", "{reading}"),
    ("ExpressionAudio", "{audio}"),
    ("SelectionText", "{popup-selection-text}"),
    ("MainDefinition", "{main-definition}"),
    (
        "Sentence",
        "{cloze-prefix}<b>{cloze-body}</b>{cloze-suffix}",
    ),
    ("Picture", "{screenshot}"),
    ("Glossary", "{glossary}"),
    ("Frequency", "{frequencies}"),
    ("FreqSort", "{frequency-harmonic-rank}"),
    ("MiscInfo", "{document-title}"),
];

/// A template for a field, guessed from its name (Lapis, Yomitan's usual
/// note types and plain ones); the first field gets the word if nothing fits.
pub fn default_template(field: &str, first: bool) -> String {
    if let Some((_, t)) = DEFAULTS.iter().find(|(f, _)| f.eq_ignore_ascii_case(field)) {
        return (*t).to_owned();
    }
    let f: String = field
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    let t = match f.as_str() {
        "word" | "term" | "vocab" | "vocabulary" | "hanzi" | "simplified" | "traditional"
        | "front" | "key" | "targetword" => "{expression}",
        "reading" | "pinyin" | "wordreading" => "{reading}",
        "definition" | "meaning" | "meanings" | "english" | "back" | "gloss" => "{main-definition}",
        "definitions" | "glossaries" => "{glossary}",
        "example" | "examplesentence" | "context" => {
            "{cloze-prefix}<b>{cloze-body}</b>{cloze-suffix}"
        }
        "frequencysort" => "{frequency-harmonic-rank}",
        "frequencies" => "{frequencies}",
        "source" | "book" => "{document-title}",
        _ if first => "{expression}",
        _ => "",
    };
    t.to_owned()
}

/// The sentence a word was found in: its characters and where the word
/// starts among them.
#[derive(Clone, Debug, PartialEq)]
pub struct Sentence {
    pub text: Vec<char>,
    pub at: usize,
}

/// Characters that end a sentence.
pub(super) fn ends(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '!' | '?' | '；' | ';' | '．')
}

/// Closing quotes and brackets: after a sentence's end, still its own.
pub(super) fn closer(c: char) -> bool {
    matches!(
        c,
        '」' | '』' | '”' | '’' | '）' | ')' | '》' | '】' | '〉' | '"' | '\''
    )
}

fn pair(open: char) -> Option<char> {
    Some(match open {
        '「' => '」',
        '『' => '』',
        '“' => '”',
        '‘' => '’',
        '（' => '）',
        '(' => ')',
        '《' => '》',
        '【' => '】',
        '〈' => '〉',
        _ => return None,
    })
}

/// The longest sentence kept on either side of the word.
const SIDE: usize = 150;

/// The sentence around `src` in a chapter's characters (blocks end at `\0`).
pub fn sentence_around(chars: &[char], src: usize) -> Option<Sentence> {
    if src >= chars.len() {
        return None;
    }
    let block = |c: char| c == '\0' || c == '\n';
    let mut start = src;
    while start > 0 && src - start < SIDE {
        let c = chars[start - 1];
        if block(c) || ends(c) {
            break;
        }
        start -= 1;
    }
    // The last sentence's closing quotes.
    while start < src && closer(chars[start]) {
        start += 1;
    }
    let mut end = src;
    while end < chars.len() && end - src < SIDE {
        let c = chars[end];
        if block(c) {
            break;
        }
        end += 1;
        if ends(c) {
            while end < chars.len() && (ends(chars[end]) || closer(chars[end])) {
                end += 1;
            }
            break;
        }
    }
    let mut text = chars[start..end].to_vec();
    let mut at = src - start;
    let lead = text
        .iter()
        .take(at)
        .take_while(|c| c.is_whitespace())
        .count();
    text.drain(..lead);
    at -= lead;
    while text.len() > at + 1 && text.last().is_some_and(|c| c.is_whitespace()) {
        text.pop();
    }
    // Quotes: a quote opened before the word and not closed in the sentence
    // starts it (what came before is outside the quote); one closed after the
    // word and not opened in the sentence ends it.
    if let Some(i) = (0..at)
        .rev()
        .find(|&i| pair(text[i]).is_some_and(|close| !text[i + 1..].contains(&close)))
    {
        text.drain(..=i);
        at -= i + 1;
    }
    if let Some(i) = (at + 1..text.len()).find(|&i| {
        let c = text[i];
        closer(c) && !matches!(c, '"' | '\'') && !text[..i].iter().any(|&o| pair(o) == Some(c))
    }) {
        text.truncate(i);
    }
    Some(Sentence { text, at })
}

/// What a card is made from: a popup entry (`group` of `lookup`) and where
/// its word was found.
#[derive(Clone)]
pub struct Source {
    pub lookup: Arc<Lookup>,
    pub group: usize,
    /// On the page: its sentence (none for words looked up in a popup).
    pub sentence: Option<Arc<Sentence>>,
}

/// Words are told apart by headword and reading (hashed: looking one up
/// while drawing allocates nothing).
pub type Key = u64;

pub fn key_of(g: &Group) -> Key {
    let (h, r) = (g.headword.as_bytes(), g.reading.as_bytes());
    let n = h.len() + 1 + r.len();
    let mut buf = [0u8; 256];
    let mut heap = Vec::new();
    let b: &mut [u8] = if n <= buf.len() {
        &mut buf[..n]
    } else {
        heap.resize(n, 0);
        &mut heap
    };
    b[..h.len()].copy_from_slice(h);
    b[h.len()] = 0;
    b[h.len() + 1..].copy_from_slice(r);
    hanstat::dict::xxh64::xxh64(b, 0)
}

/// What the book and dictionaries say, for filling templates.
struct Fill<'a> {
    g: &'a Group,
    sentence: Option<&'a Sentence>,
    titles: &'a [String],
    book: &'a str,
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    hanstat::dict::sc::escape(s, &mut o);
    o
}

/// A glossary's HTML for Anki: lookup links become plain text (they lead
/// nowhere there) and images go (their files are the dictionary's).
fn for_anki(html: &str) -> String {
    static LINK: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"<a href="[^"]*""#).unwrap());
    static IMG: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"<img [^>]*>"#).unwrap());
    let s = LINK.replace_all(html, "<span");
    let s = s.replace("</a>", "</span>");
    IMG.replace_all(&s, "").into_owned()
}

impl Fill<'_> {
    /// The glossaries as Yomitan writes `{glossary}`: each with its
    /// dictionary's name (and tags) in front, several as a numbered list.
    fn glossary(&self, first_only: bool) -> String {
        let mut items = Vec::new();
        let parts = if first_only {
            &self.g.parts[..self.g.parts.len().min(1)]
        } else {
            &self.g.parts[..]
        };
        for p in parts {
            let title = self.titles.get(p.dict).map_or("", String::as_str);
            for gl in &p.glossaries {
                let Ok(json) = &gl.json else { continue };
                let mut body = String::new();
                if hanstat::dict::sc::glossary(json, &mut body).is_err() {
                    continue;
                }
                let tags: Vec<&str> = gl
                    .def_tags
                    .split_whitespace()
                    .chain(gl.term_tags.split_whitespace())
                    .chain(std::iter::once(title).filter(|t| !t.is_empty()))
                    .collect();
                items.push((
                    title,
                    format!(
                        "<i>({})</i> <span>{}</span>",
                        esc(&tags.join(", ")),
                        for_anki(&body)
                    ),
                ));
            }
        }
        if items.is_empty() {
            return String::new();
        }
        let mut out = String::from(r#"<div style="text-align: left;" class="yomitan-glossary">"#);
        if items.len() == 1 {
            out.push_str(&format!(
                r#"<div data-dictionary="{}">{}</div>"#,
                esc(items[0].0),
                items[0].1
            ));
        } else {
            out.push_str("<ol>");
            for (t, h) in &items {
                out.push_str(&format!(r#"<li data-dictionary="{}">{h}</li>"#, esc(t)));
            }
            out.push_str("</ol>");
        }
        out.push_str("</div>");
        out
    }

    /// (before, word, after) as the book writes them.
    fn cloze(&self) -> (String, String, String) {
        match self.sentence {
            Some(s) => {
                let at = s.at.min(s.text.len());
                let end = (at + self.g.matched).min(s.text.len());
                (
                    s.text[..at].iter().collect(),
                    s.text[at..end].iter().collect(),
                    s.text[end..].iter().collect(),
                )
            }
            None => (String::new(), String::new(), String::new()),
        }
    }

    fn marker(&self, m: &str) -> Option<String> {
        Some(match m {
            "expression" => esc(&self.g.headword),
            "reading" if self.g.reading != self.g.headword => esc(&self.g.reading),
            "reading" => String::new(),
            "glossary" => self.glossary(false),
            "glossary-first" | "main-definition" => self.glossary(true),
            "sentence" => {
                let (a, b, c) = self.cloze();
                if b.is_empty() {
                    String::new()
                } else {
                    format!("{}<b>{}</b>{}", esc(&a), esc(&b), esc(&c))
                }
            }
            "cloze-prefix" => esc(&self.cloze().0),
            "cloze-body" => esc(&self.cloze().1),
            "cloze-suffix" => esc(&self.cloze().2),
            "document-title" => esc(self.book),
            // Nothing in a book to fill them with: empty, not the marker.
            "audio" | "screenshot" | "popup-selection-text" => String::new(),
            "frequencies" => {
                let items: Vec<String> = self
                    .g
                    .freqs
                    .iter()
                    .filter(|b| !b.values.is_empty())
                    .map(|b| {
                        let v: Vec<&str> = b.values.iter().map(|v| v.display.as_str()).collect();
                        format!(
                            "<li>{}: {}</li>",
                            esc(self.titles.get(b.dict).map_or("", String::as_str)),
                            esc(&v.join(", "))
                        )
                    })
                    .collect();
                if items.is_empty() {
                    String::new()
                } else {
                    format!(r#"<ul style="text-align: left;">{}</ul>"#, items.concat())
                }
            }
            "frequency-harmonic-rank" => {
                let ranks: Vec<f64> = self
                    .g
                    .freqs
                    .iter()
                    .filter_map(|b| b.values.first())
                    .filter(|v| v.value > 0)
                    .map(|v| v.value as f64)
                    .collect();
                if ranks.is_empty() {
                    "9999999".into()
                } else {
                    let h = ranks.len() as f64 / ranks.iter().map(|r| 1.0 / r).sum::<f64>();
                    (h.floor() as i64).to_string()
                }
            }
            _ => return None,
        })
    }

    /// `template` with its markers filled; unknown ones stay as written.
    fn apply(&self, template: &str) -> String {
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(i) = rest.find('{') {
            out.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            match after.find('}') {
                Some(j) if !after[..j].contains('{') => {
                    match self.marker(&after[..j]) {
                        Some(v) => out.push_str(&v),
                        None => out.push_str(&rest[i..i + j + 2]),
                    }
                    rest = &after[j + 1..];
                }
                _ => {
                    out.push('{');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }
}

/// The note for `src`; `first_only`: just its first field (enough for
/// Anki's duplicate check, and cheaper).
fn note(
    cfg: &CardSettings,
    src: &Source,
    titles: &[String],
    book: &str,
    first_only: bool,
) -> Option<Note> {
    let g = src.lookup.groups.get(src.group)?;
    let f = Fill {
        g,
        sentence: src.sentence.as_deref(),
        titles,
        book,
    };
    let n = if first_only { 1 } else { cfg.fields.len() };
    Some(Note {
        deck: cfg.deck.clone(),
        model: cfg.model.clone(),
        fields: cfg
            .fields
            .iter()
            .take(n)
            .map(|(name, t)| (name.clone(), f.apply(t)))
            .collect(),
        tags: cfg.tags.split_whitespace().map(str::to_owned).collect(),
        check_all_models: cfg.check_all_models,
    })
}

/// Where a word stands with Anki.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Being checked (or not yet).
    Checking,
    CanAdd,
    /// Already in Anki: the button is greyed out.
    Duplicate,
    Adding,
    /// Added in this session: the note's id.
    Added(i64),
    /// Anki couldn't be asked, or said no (the reason).
    Failed(String),
}

/// What the setup window lists.
#[derive(Clone, Debug, Default)]
pub struct Lists {
    pub version: u64,
    pub decks: Vec<String>,
    pub models: Vec<String>,
}

enum Job {
    Check {
        cfg: Arc<CardSettings>,
        generation: u64,
        level: usize,
        lookup: Arc<Lookup>,
        sentence: Option<Arc<Sentence>>,
        titles: Arc<Vec<String>>,
        book: Arc<str>,
    },
    Add {
        cfg: Arc<CardSettings>,
        generation: u64,
        src: Source,
        titles: Arc<Vec<String>>,
        book: Arc<str>,
    },
    Browse {
        cfg: Arc<CardSettings>,
        id: i64,
    },
    Lists {
        cfg: Arc<CardSettings>,
    },
    Fields {
        cfg: Arc<CardSettings>,
        model: String,
    },
}

enum Answer {
    /// What a check found.
    Checked(u64, Vec<(Key, Status)>),
    /// How an add went.
    AddResult(u64, Key, Status),
    Lists(Result<Lists, String>),
    Fields(String, Result<Vec<String>, String>),
    Browsed(Result<(), String>),
}

type ClientSlot = Option<((String, String), Client)>;

/// The client for `cfg`'s address and key (made again when they change).
fn client_for<'a>(slot: &'a mut ClientSlot, cfg: &CardSettings) -> &'a Client {
    let k = (cfg.url.clone(), cfg.key.clone());
    if slot.as_ref().is_none_or(|(ck, _)| *ck != k) {
        *slot = Some((k.clone(), Client::new(&cfg.url, Some(&cfg.key))));
    }
    &slot.as_ref().unwrap().1
}

fn worker(rx: Receiver<Job>, tx: Sender<Answer>, repaint: Arc<dyn Fn() + Send + Sync>) {
    let mut client: ClientSlot = None;
    while let Ok(first) = rx.recv() {
        // Checks for a popup since replaced are dropped: only the latest per
        // popup level is answered.
        let mut jobs = vec![first];
        jobs.extend(rx.try_iter());
        let mut latest: HashMap<usize, usize> = HashMap::new();
        for (i, j) in jobs.iter().enumerate() {
            if let Job::Check { level, .. } = j {
                latest.insert(*level, i);
            }
        }
        for (i, job) in jobs.into_iter().enumerate() {
            let answer = match job {
                Job::Check {
                    cfg,
                    generation,
                    level,
                    lookup,
                    sentence,
                    titles,
                    book,
                } => {
                    if latest.get(&level) != Some(&i) {
                        continue;
                    }
                    let mut keys = Vec::new();
                    let mut notes = Vec::new();
                    for group in 0..lookup.groups.len() {
                        let s = Source {
                            lookup: lookup.clone(),
                            group,
                            sentence: sentence.clone(),
                        };
                        if let Some(n) = note(&cfg, &s, &titles, &book, true) {
                            keys.push(key_of(&lookup.groups[group]));
                            notes.push(n);
                        }
                    }
                    let st = match client_for(&mut client, &cfg).can_add(&notes) {
                        Ok(r) => keys
                            .into_iter()
                            .zip(r)
                            .map(|(k, c)| {
                                (
                                    k,
                                    match c {
                                        CanAdd::Yes => Status::CanAdd,
                                        CanAdd::Duplicate => Status::Duplicate,
                                        CanAdd::No(e) => Status::Failed(e),
                                    },
                                )
                            })
                            .collect(),
                        Err(e) => keys
                            .into_iter()
                            .map(|k| (k, Status::Failed(e.clone())))
                            .collect(),
                    };
                    Answer::Checked(generation, st)
                }
                Job::Add {
                    cfg,
                    generation,
                    src,
                    titles,
                    book,
                } => {
                    let Some(n) = note(&cfg, &src, &titles, &book, false) else {
                        continue;
                    };
                    let k = key_of(&src.lookup.groups[src.group]);
                    let st = match client_for(&mut client, &cfg).add_note(&n) {
                        Ok(id) => Status::Added(id),
                        Err(e) if e.contains("duplicate") => Status::Duplicate,
                        Err(e) => Status::Failed(e),
                    };
                    Answer::AddResult(generation, k, st)
                }
                Job::Browse { cfg, id } => {
                    Answer::Browsed(client_for(&mut client, &cfg).gui_browse(&format!("nid:{id}")))
                }
                Job::Lists { cfg } => {
                    let c = client_for(&mut client, &cfg);
                    Answer::Lists((|| {
                        Ok(Lists {
                            version: c.version()?,
                            decks: c.deck_names()?,
                            models: c.model_names()?,
                        })
                    })())
                }
                Job::Fields { cfg, model } => {
                    let r = client_for(&mut client, &cfg).model_field_names(&model);
                    Answer::Fields(model, r)
                }
            };
            if tx.send(answer).is_err() {
                return;
            }
            repaint();
        }
    }
}

/// The popup's side: statuses by word, and the worker.
pub struct Cards {
    jobs: Sender<Job>,
    answers: Receiver<Answer>,
    pub cfg: Arc<CardSettings>,
    /// Bumped when the settings change: answers for older ones are dropped.
    generation: u64,
    statuses: HashMap<Key, Status>,
    titles: Arc<Vec<String>>,
    pub book: Arc<str>,
    /// The setup window.
    pub setup_open: bool,
    /// Answers for the setup window.
    pub lists: Option<Result<Lists, String>>,
    pub fields_of: Option<(String, Result<Vec<String>, String>)>,
    /// The last error from opening a note in Anki's browser.
    pub browse_error: Option<String>,
}

impl Cards {
    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Cards {
        let (jobs, jobs_rx) = channel();
        let (answers_tx, answers) = channel();
        let _ = std::thread::Builder::new()
            .name("anki-cards".into())
            .spawn(move || worker(jobs_rx, answers_tx, repaint));
        Cards {
            jobs,
            answers,
            cfg: Arc::new(CardSettings::default()),
            generation: 0,
            statuses: HashMap::new(),
            titles: Arc::new(Vec::new()),
            book: Arc::from(""),
            setup_open: false,
            lists: None,
            fields_of: None,
            browse_error: None,
        }
    }

    /// The **+** shows in the popup.
    pub fn shown(&self) -> bool {
        self.cfg.enabled
    }

    /// Settings as now: `true` if they changed (the words are checked again).
    pub fn set_settings(&mut self, s: &CardSettings) -> bool {
        if *self.cfg == *s {
            return false;
        }
        self.cfg = Arc::new(s.clone());
        self.generation += 1;
        self.statuses.clear();
        true
    }

    pub fn set_dicts(&mut self, dicts: &[DictInfo]) {
        self.titles = Arc::new(dicts.iter().map(|d| d.title.clone()).collect());
    }

    pub fn status(&self, g: &Group) -> Option<&Status> {
        self.statuses.get(&key_of(g))
    }

    /// Check the entries of popup `level` (a channel send; Anki answers on
    /// the worker).
    pub fn check(&mut self, level: usize, lookup: &Arc<Lookup>, sentence: Option<&Arc<Sentence>>) {
        if !self.cfg.enabled || !self.cfg.ready() {
            return;
        }
        for g in &lookup.groups {
            self.statuses.entry(key_of(g)).or_insert(Status::Checking);
        }
        let _ = self.jobs.send(Job::Check {
            cfg: self.cfg.clone(),
            generation: self.generation,
            level,
            lookup: lookup.clone(),
            sentence: sentence.cloned(),
            titles: self.titles.clone(),
            book: self.book.clone(),
        });
    }

    pub fn add(&mut self, src: Source) {
        let Some(g) = src.lookup.groups.get(src.group) else {
            return;
        };
        self.statuses.insert(key_of(g), Status::Adding);
        let _ = self.jobs.send(Job::Add {
            cfg: self.cfg.clone(),
            generation: self.generation,
            src,
            titles: self.titles.clone(),
            book: self.book.clone(),
        });
    }

    pub fn browse(&mut self, id: i64) {
        let _ = self.jobs.send(Job::Browse {
            cfg: self.cfg.clone(),
            id,
        });
    }

    /// Ask for the decks and note types (and the chosen type's fields).
    pub fn refresh_lists(&mut self, s: &CardSettings) {
        let cfg = Arc::new(s.clone());
        self.lists = None;
        let _ = self.jobs.send(Job::Lists { cfg: cfg.clone() });
        if !s.model.is_empty() {
            self.ask_fields(s);
        }
    }

    pub fn ask_fields(&mut self, s: &CardSettings) {
        self.fields_of = None;
        let _ = self.jobs.send(Job::Fields {
            cfg: Arc::new(s.clone()),
            model: s.model.clone(),
        });
    }

    /// Take in the worker's answers (every frame).
    pub fn poll(&mut self) {
        while let Ok(a) = self.answers.try_recv() {
            match a {
                Answer::Checked(generation, st) if generation == self.generation => {
                    for (k, s) in st {
                        // A check never overrides an add under way or done
                        // (it may have been asked before the click).
                        if !matches!(
                            self.statuses.get(&k),
                            Some(Status::Adding | Status::Added(_))
                        ) {
                            self.statuses.insert(k, s);
                        }
                    }
                }
                Answer::AddResult(generation, k, s) if generation == self.generation => {
                    self.statuses.insert(k, s);
                }
                Answer::Checked(..) | Answer::AddResult(..) => {}
                Answer::Lists(r) => self.lists = Some(r),
                Answer::Fields(m, r) => self.fields_of = Some((m, r)),
                Answer::Browsed(r) => self.browse_error = r.err(),
            }
        }
    }

    /// The setup window; returns whether the settings changed.
    pub fn window(&mut self, ctx: &eframe::egui::Context, s: &mut CardSettings) -> bool {
        use eframe::egui::{self, RichText};
        if !self.setup_open {
            return false;
        }
        let before = s.clone();
        let faint = super::theme::BASE1;
        let red = egui::Color32::from_rgb(0xdc, 0x32, 0x2f);
        // Fields arrived for the chosen note type: its list, templates kept
        // for fields of the same name, guessed for the others.
        if let Some((m, Ok(names))) = &self.fields_of
            && *m == s.model
        {
            let names_now: Vec<&String> = s.fields.iter().map(|(n, _)| n).collect();
            if names_now != names.iter().collect::<Vec<_>>() {
                s.fields = names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| {
                        let t = s
                            .fields
                            .iter()
                            .find(|(f, _)| f == n)
                            .map_or_else(|| default_template(n, i == 0), |(_, t)| t.clone());
                        (n.clone(), t)
                    })
                    .collect();
            }
        }
        let mut open = self.setup_open;
        let mut refresh = false;
        let mut model_changed = false;
        egui::Window::new("Anki cards")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(620.0)
            .max_height(ctx.content_rect().height() * 0.9)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "The popup's + adds a note for its word to Anki through AnkiConnect: Anki must be running, \
                         with the AnkiConnect add-on (2055492159) or one that works like it installed. \
                         The + is greyed out when the word is already in Anki.",
                    )
                    .color(super::theme::BASE00),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    match &self.lists {
                        None => {
                            ui.spinner();
                            ui.label("Asking Anki…");
                        }
                        Some(Ok(l)) => {
                            ui.label(format!("Connected to AnkiConnect (API {})", l.version));
                        }
                        Some(Err(e)) => {
                            ui.label(RichText::new(e).color(red));
                        }
                    }
                    if ui.button("Refresh").on_hover_text("Ask Anki again for its decks and note types (after changing the address or key)").clicked() {
                        refresh = true;
                    }
                });
                ui.checkbox(&mut s.enabled, "Show the + button in the popup");
                ui.add_space(4.0);
                let lists = self.lists.as_ref().and_then(|l| l.as_ref().ok());
                egui::Grid::new("anki-cards").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Deck");
                    choose(ui, "anki-deck", &mut s.deck, lists.map(|l| &l.decks[..]));
                    ui.end_row();
                    ui.label("Note type");
                    let m = s.model.clone();
                    choose(ui, "anki-model", &mut s.model, lists.map(|l| &l.models[..]));
                    if s.model != m {
                        model_changed = true;
                    }
                    ui.end_row();
                    ui.label("Tags");
                    ui.add(egui::TextEdit::singleline(&mut s.tags).desired_width(300.0))
                        .on_hover_text("Separated by spaces");
                    ui.end_row();
                    ui.label("Duplicates");
                    ui.checkbox(&mut s.check_all_models, "Notes of every type count")
                        .on_hover_text(
                            "A word is already in Anki when a note's first field is the same. Off: only notes of this \
                             type count, as in Yomitan.",
                        );
                    ui.end_row();
                });
                ui.add_space(6.0);
                ui.label(RichText::new("Fields").strong());
                match &self.fields_of {
                    Some((m, Err(e))) if *m == s.model => {
                        ui.label(RichText::new(e).color(red));
                    }
                    None if !s.model.is_empty() && s.fields.is_empty() => {
                        ui.spinner();
                    }
                    _ => {}
                }
                if s.model.is_empty() {
                    ui.label(RichText::new("Choose a note type.").color(faint));
                }
                egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
                    egui::Grid::new("anki-fields").num_columns(3).spacing([10.0, 4.0]).striped(true).show(ui, |ui| {
                        for (i, (name, t)) in s.fields.iter_mut().enumerate() {
                            let l = ui.label(name.as_str());
                            if i == 0 {
                                l.on_hover_text("The first field: what Anki compares to find duplicates");
                            }
                            ui.add_sized([340.0, ui.spacing().interact_size.y], egui::TextEdit::singleline(t));
                            ui.menu_button("+", |ui| {
                                for (m, what) in MARKERS {
                                    if ui.button(*m).on_hover_text(*what).clicked() {
                                        t.push_str(m);
                                        ui.close();
                                    }
                                }
                            })
                            .response
                            .on_hover_text("Insert a marker");
                            ui.end_row();
                        }
                    });
                });
                if !s.fields.is_empty() && s.fields[0].1.trim().is_empty() {
                    ui.label(RichText::new("The first field needs a template: Anki won't add a note with it empty.").color(red));
                }
                egui::CollapsingHeader::new("Markers").show(ui, |ui| {
                    for (m, what) in MARKERS {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(*m).monospace());
                            ui.label(RichText::new(*what).color(super::theme::BASE00));
                        });
                    }
                });
                egui::CollapsingHeader::new("Connection").show(ui, |ui| {
                    egui::Grid::new("anki-conn").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        ui.label("Address");
                        ui.add(egui::TextEdit::singleline(&mut s.url).desired_width(260.0));
                        ui.end_row();
                        ui.label("API key");
                        ui.add(egui::TextEdit::singleline(&mut s.key).password(true).desired_width(260.0))
                            .on_hover_text("Only if one is set in AnkiConnect's config");
                        ui.end_row();
                    });
                });
            });
        self.setup_open = open;
        if model_changed && !s.model.is_empty() {
            s.fields.clear();
            self.ask_fields(s);
        }
        // Asked again on Refresh only: not on every key typed in the address.
        if refresh {
            self.refresh_lists(s);
        }
        // First time: a likely deck and note type.
        let mut picked_model = false;
        if let Some(Ok(l)) = &self.lists {
            if s.model.is_empty()
                && let Some(m) = l.models.iter().find(|m| *m == "Lapis Chinese")
            {
                s.model = m.clone();
                picked_model = true;
            }
            if s.deck.is_empty() && !l.decks.is_empty() {
                s.deck = l
                    .decks
                    .iter()
                    .find(|d| *d != "Default")
                    .unwrap_or(&l.decks[0])
                    .clone();
            }
        }
        if picked_model {
            self.ask_fields(s);
        }
        *s != before
    }

    /// Open the setup window (and ask Anki for its lists).
    pub fn open_setup(&mut self) {
        if !self.setup_open {
            self.setup_open = true;
            let cfg = self.cfg.clone();
            self.refresh_lists(&cfg);
        }
    }
}

/// A combo box of `names` (or a text box while they aren't known).
fn choose(ui: &mut eframe::egui::Ui, id: &str, value: &mut String, names: Option<&[String]>) {
    use eframe::egui;
    match names {
        Some(names) => {
            let shown = if value.is_empty() {
                "Choose…"
            } else {
                value.as_str()
            }
            .to_owned();
            egui::ComboBox::from_id_salt(id)
                .selected_text(shown)
                .width(300.0)
                .height(400.0)
                .show_ui(ui, |ui| {
                    for n in names {
                        ui.selectable_value(value, n.clone(), n);
                    }
                });
        }
        None => {
            ui.add(egui::TextEdit::singleline(value).desired_width(300.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hanstat::dict::lookup::{FreqBadge, Gloss, Part};
    use hanstat::dict::store::Freq;

    fn s(text: &str, word: &str) -> (String, String) {
        let c: Vec<char> = text.chars().collect();
        let w: Vec<char> = word.chars().collect();
        let src = (0..c.len()).find(|&i| c[i..].starts_with(&w)).unwrap();
        let s = sentence_around(&c, src).unwrap();
        let t: String = s.text.iter().collect();
        let at: String = s.text[s.at..].iter().take(w.len()).collect();
        (t, at)
    }

    #[test]
    fn sentences() {
        let text =
            "他走了。若是平常的话，这时候我应该要为了明天而养精蓄锐，准备上床睡觉了。\0下一段";
        assert_eq!(
            s(text, "养精蓄锐").0,
            "若是平常的话，这时候我应该要为了明天而养精蓄锐，准备上床睡觉了。"
        );
        assert_eq!(s(text, "养精蓄锐").1, "养精蓄锐");
        assert_eq!(s(text, "下一").0, "下一段");
        // Quotes: a whole quoted sentence keeps its quotes; a closing quote
        // after the end goes with it.
        let q = "　　「你说什么？」他问。\0";
        assert_eq!(s(q, "什么").0, "「你说什么？」");
        assert_eq!(s(q, "他问").0, "他问。");
        // A quote opened earlier and not closed here goes.
        let q = "「我想，明天再说吧。你呢？」";
        assert_eq!(s(q, "明天").0, "我想，明天再说吧。");
        assert_eq!(s(q, "明天").1, "明天");
        // A quote closed later and not opened here goes.
        assert_eq!(s("「我想。明天再说吧」他说", "明天").0, "明天再说吧");
    }

    fn group() -> Group {
        Group {
            headword: "养精蓄锐".into(),
            reading: "yǎng jīng xù ruì".into(),
            matched: 4,
            folded: false,
            freqs: vec![
                FreqBadge {
                    dict: 1,
                    values: vec![Freq {
                        reading: String::new(),
                        value: 40000,
                        display: "40000".into(),
                    }],
                    folded: false,
                },
                FreqBadge {
                    dict: 2,
                    values: vec![Freq {
                        reading: String::new(),
                        value: 20000,
                        display: "20000".into(),
                    }],
                    folded: false,
                },
            ],
            parts: vec![Part {
                dict: 0,
                glossaries: vec![Gloss {
                    json: Ok(r#"["to conserve energy", {"type":"structured-content","content":{"tag":"a","href":"?query=x","content":"x"}}]"#.into()),
                    def_tags: String::new(),
                    term_tags: String::new(),
                    score: 0,
                }],
            }],
        }
    }

    #[test]
    fn templates() {
        let g = group();
        let sentence = Sentence {
            text: "为了明天而养精蓄锐。".chars().collect(),
            at: 5,
        };
        let titles = vec![
            "CC-CEDICT".to_owned(),
            "BLCU".to_owned(),
            "SUBTLEX<".to_owned(),
        ];
        let f = Fill {
            g: &g,
            sentence: Some(&sentence),
            titles: &titles,
            book: "通往夏天的隧道",
        };
        assert_eq!(f.apply("{expression}"), "养精蓄锐");
        assert_eq!(f.apply("{sentence}"), "为了明天而<b>养精蓄锐</b>。");
        assert_eq!(
            f.apply("{cloze-prefix}|{cloze-body}|{cloze-suffix}"),
            "为了明天而|养精蓄锐|。"
        );
        assert_eq!(
            f.apply("{document-title} | hanstat"),
            "通往夏天的隧道 | hanstat"
        );
        assert_eq!(
            f.apply("{cloze-prefix}<b>{cloze-body}</b>{cloze-suffix}"),
            f.apply("{sentence}")
        );
        assert_eq!(f.apply("[{audio}{screenshot}{popup-selection-text}]"), "[]");
        assert_eq!(f.apply("{main-definition}"), f.apply("{glossary-first}"));
        // 2 / (1/40000 + 1/20000)
        assert_eq!(f.apply("{frequency-harmonic-rank}"), "26666");
        assert!(
            f.apply("{frequencies}")
                .contains("<li>SUBTLEX&lt;: 20000</li>")
        );
        let gl = f.apply("{glossary}");
        assert!(gl.starts_with(r#"<div style="text-align: left;" class="yomitan-glossary"><div data-dictionary="CC-CEDICT"><i>(CC-CEDICT)</i>"#), "{gl}");
        assert!(
            gl.contains("to conserve energy") && !gl.contains("<a "),
            "links become text: {gl}"
        );
        // Unknown markers and stray braces stay as written.
        assert_eq!(f.apply("{nope} {x {expression}}"), "{nope} {x 养精蓄锐}");
        // In a popup, no sentence: empty.
        let f = Fill {
            sentence: None,
            ..f
        };
        assert_eq!(f.apply("[{sentence}]"), "[]");
    }

    #[test]
    fn field_guesses() {
        let lapis = [
            "Expression",
            "ExpressionReading",
            "ExpressionAudio",
            "SelectionText",
            "MainDefinition",
            "DefinitionPicture",
            "Sentence",
            "SentenceAudio",
            "Picture",
            "Glossary",
            "Hint",
            "IsWordAndSentenceCard",
            "IsClickCard",
            "IsSentenceCard",
            "IsAudioCard",
            "Frequency",
            "FreqSort",
            "MiscInfo",
        ];
        let got: Vec<(String, String)> = lapis
            .iter()
            .enumerate()
            .map(|(i, f)| (f.to_string(), default_template(f, i == 0)))
            .filter(|(_, t)| !t.is_empty())
            .collect();
        // As the user's Yomitan has them for Lapis.
        let want = [
            ("Expression", "{expression}"),
            ("ExpressionReading", "{reading}"),
            ("ExpressionAudio", "{audio}"),
            ("SelectionText", "{popup-selection-text}"),
            ("MainDefinition", "{main-definition}"),
            (
                "Sentence",
                "{cloze-prefix}<b>{cloze-body}</b>{cloze-suffix}",
            ),
            ("Picture", "{screenshot}"),
            ("Glossary", "{glossary}"),
            ("Frequency", "{frequencies}"),
            ("FreqSort", "{frequency-harmonic-rank}"),
            ("MiscInfo", "{document-title}"),
        ];
        assert_eq!(
            got,
            want.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect::<Vec<_>>()
        );
        assert_eq!(default_template("Front", true), "{expression}");
        assert_eq!(default_template("Notes", true), "{expression}");
        assert_eq!(default_template("Notes", false), "");
    }
}
