//! The reading path: an order for a library of books in one language, in
//! which the words studied before each book are known for every book after
//! it, so that the hardest step is as easy as it can be made.
//!
//! **The model.** The learner starts from what they know now (Anki, levels,
//! word lists, marked words). Before reading a book they study some of its
//! unknown words, either until it reaches a target share of running words
//! known ([`Study::Target`]) or a fixed number of them ([`Study::Fixed`]).
//! Those words stay known. Optionally, an unknown word read at least M times
//! in a book counts as learned from reading it ([`Params::incidental`]).
//! Names (words containing a name or term of the book's profile) are never
//! studied and, unless already known, are left out of the running words, as
//! on the Overview.
//!
//! **Which words.** A book's study words are its unknown words in order of
//! their value: their share of this book's running words, plus
//! [`Params::lookahead`] times their share of the running words of every
//! book still to come. At 0 that is the book's most frequent unknown words,
//! the fewest that reach a target; above 0 it prefers words the rest of the
//! library needs too.
//!
//! **Which order.** With a target, each step costs the number of words
//! studied, and orders are compared by their largest step, then by the
//! words studied in all. With a fixed number, the words studied are fixed
//! and orders are compared by the lowest share of running words known while
//! reading, then by the average. (Comparing by the largest step, then the
//! next largest and so on, was tried and rejected: one book needing many
//! words wherever it goes sets the largest step, and the search then traded
//! words in all for slightly smaller second and third steps, ending above
//! reading the easiest book first.) Finding the best order is
//! a scheduling problem with no shortcut (what is learned depends on the
//! order), so [`Library::optimise`] runs a beam search: it keeps the best
//! `width` partial orders at each length, ranking the books that could come
//! next by the cheap frequency-order cost and computing the real step for
//! the most promising. Width 1 is greedy: always the easiest book next.
//!
//! **Series.** Books whose titles name one series ([`crate::series`]:
//! `X（第二部）`, `X 2`, `X Vol. 3`, `X（下）` …) are read in volume order,
//! with other books between them where that is better: the search only
//! offers a volume once the earlier ones on the path are read, and
//! easiest-first and orders set by hand are put in series order
//! ([`Library::in_series_order`]).
//!
//! **Keeping series together** ([`Params::keep_series`]). Each series is
//! one block: its volumes are read one after another, in volume order, and
//! the search places the block where it is best overall, with what earlier
//! volumes teach counted for the later ones (it compares a path that has
//! just read the whole block with the other paths of as many books).
//!
//! **Writing style.** With ratings on ([`crate::style`]), a book whose prose
//! is harder than typical gets a higher target (or, under a fixed number,
//! counts as read at a lower coverage), and an easier one a lower:
//! [`LibBook::offset`] percentage points.

use crate::anki::Knowledge;
use crate::profile::Profile;
use crate::series;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub mod stats;

/// How much to study before each book.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Study {
    /// Until this percentage of the book's running words is known.
    Target(f64),
    /// This many of its unknown words.
    Fixed(usize),
    /// Nothing: learn while reading. A book teaches the `percent`% most
    /// frequent of the words it uses at least `min_count` times, as ✔ Mark
    /// read takes them ([`crate::known::read_selection`]), and a word met in
    /// two or more books, `min_count` times in all
    /// ([`crate::known::across_books`]).
    Read { percent: u8, min_count: u32 },
}

/// Running words at the start of a book that "at the start" is taken over
/// ([`Step::start`]): roughly its first few chapters.
pub const OPENING: u64 = 20_000;

/// The Reading path's two ways of reading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Nothing studied: books teach their frequent words ([`Study::Read`]).
    #[default]
    Read,
    /// Study before each book ([`Study::Target`], [`Study::Fixed`]).
    Study,
}

/// What makes one order better than another.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Objective {
    /// The fewest words studied in all (under a fixed number: the highest
    /// average coverage while reading), then the smallest largest step.
    Total,
    /// The smallest largest step (fixed number: the highest lowest
    /// coverage), then the fewest words in all.
    #[default]
    Peak,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub study: Study,
    pub objective: Objective,
    /// After reading a book, its unknown words read at least this many times
    /// count as learned.
    pub incidental: Option<u32>,
    /// Weight of the books still to come when choosing study words (0: this
    /// book's most frequent unknown words).
    pub lookahead: f32,
    /// Read every series' volumes one after another, with no other book
    /// between them (the module documentation, **Keeping series together**).
    pub keep_series: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            study: Study::Target(95.0),
            objective: Objective::default(),
            incidental: None,
            lookahead: DEFAULT_LOOKAHEAD,
            keep_series: false,
        }
    }
}

/// Measured on 69 JJWXC novels, for learners knowing HSK 1–3, 1–5 and 1–9
/// and targets of 95% and 98% (`examples/plan_eval.rs`): the words studied
/// in all are lowest at about this weight (20–26% below 0, which studies each
/// book's most frequent unknown words), and the largest step keeps falling
/// slowly beyond it while the total rises again.
pub const DEFAULT_LOOKAHEAD: f32 = 2.0;

/// The Reading path's choices, saved in `plan.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Learn while reading, or study before each book.
    pub mode: Mode,
    /// What makes one order better than another, in either mode.
    pub objective: Objective,
    /// Study a fixed number of words before each book instead of to a target.
    pub fixed: bool,
    pub target: f64,
    pub learn: usize,
    pub incidental_on: bool,
    pub incidental: u32,
    pub lookahead: f32,
    /// Marking a book read: the share of its words occurring at least
    /// `read_min_count` times that count as known
    /// ([`crate::known::read_selection`]), as last chosen.
    pub read_percent: u8,
    pub read_min_count: u32,
    /// Keep each series' books together, in either mode ([`Params::keep_series`]).
    pub keep_series: bool,
    /// History ids of the books left out of the path.
    pub left_out: Vec<String>,
    /// The reading order set by hand (History ids), under the key
    /// [`MANUAL_KEY`] (files from builds that also read Korean and Japanese
    /// books kept one per language); without one the planner orders the books.
    pub manual: HashMap<String, Vec<String>>,
}

/// The key of the hand-set order in [`Settings::manual`].
const MANUAL_KEY: &str = "Chinese";

impl Default for Settings {
    fn default() -> Self {
        Settings {
            mode: Mode::Read,
            objective: Objective::Peak,
            fixed: false,
            target: 95.0,
            learn: 300,
            incidental_on: false,
            incidental: 10,
            lookahead: DEFAULT_LOOKAHEAD,
            read_percent: 90,
            read_min_count: 10,
            keep_series: false,
            left_out: Vec::new(),
            manual: HashMap::new(),
        }
    }
}

impl Settings {
    pub fn path() -> std::path::PathBuf {
        crate::models::app_dir().join("plan.json")
    }

    /// Missing → defaults; unreadable → defaults and a warning.
    pub fn load() -> (Settings, Option<String>) {
        // An unreadable file is kept aside and reported by the saver.
        (
            crate::saver::load_json(&Self::path()).unwrap_or_default(),
            None,
        )
    }

    pub fn save(&self) -> Result<(), String> {
        crate::saver::write(&Self::path(), serde_json::to_vec_pretty(self).unwrap())
    }

    pub fn params(&self) -> Params {
        match self.mode {
            Mode::Read => Params {
                study: Study::Read {
                    percent: self.read_percent,
                    min_count: self.read_min_count,
                },
                objective: self.objective,
                incidental: None,
                lookahead: 0.0,
                keep_series: self.keep_series,
            },
            Mode::Study => Params {
                study: if self.fixed {
                    Study::Fixed(self.learn)
                } else {
                    Study::Target(self.target)
                },
                objective: self.objective,
                incidental: self.incidental_on.then_some(self.incidental),
                lookahead: self.lookahead,
                keep_series: self.keep_series,
            },
        }
    }

    /// The hand-set order, if any.
    pub fn manual_order(&self) -> Option<&Vec<String>> {
        self.manual.get(MANUAL_KEY)
    }

    pub fn set_manual_order(&mut self, ids: Vec<String>) {
        self.manual.insert(MANUAL_KEY.to_owned(), ids);
    }

    pub fn clear_manual_order(&mut self) {
        self.manual.remove(MANUAL_KEY);
    }
}

/// A set of word ids, and (learning while reading) how often each was met
/// in the books read so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Known {
    bits: Vec<u64>,
    /// By word id, saturating; empty until a meeting is added.
    met: Vec<u16>,
}

impl Known {
    fn new(n: usize) -> Known {
        Known {
            bits: vec![0; n.div_ceil(64)],
            met: Vec::new(),
        }
    }
    #[inline]
    pub fn has(&self, w: u32) -> bool {
        self.bits[(w >> 6) as usize] >> (w & 63) & 1 == 1
    }
    #[inline]
    fn add(&mut self, w: u32) {
        self.bits[(w >> 6) as usize] |= 1 << (w & 63);
    }
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.bits.iter().map(|x| x.count_ones() as usize).sum()
    }
    /// Times word `w` was met in the books read.
    #[inline]
    pub fn met(&self, w: u32) -> u32 {
        self.met.get(w as usize).map_or(0, |&m| m as u32)
    }
    fn add_met(&mut self, w: u32, n: u32) {
        if self.met.is_empty() {
            self.met = vec![0; self.bits.len() * 64];
        }
        let m = &mut self.met[w as usize];
        *m = (*m as u32 + n).min(u16::MAX as u32) as u16;
    }
}

/// One book of the library, its words numbered across the library. Its word
/// data is shared, so a copy of the library (to set offsets or places) is
/// cheap.
#[derive(Clone, Debug)]
pub struct LibBook {
    pub id: String,
    pub title: String,
    pub segmenter: String,
    #[cfg(test)]
    pub variant: String,
    /// The series it belongs to (numbered within the library) and its
    /// volume ([`series::Volume::number`]), if another book of the library
    /// is in that series.
    pub series: Option<(u32, Vec<u32>)>,
    /// Percentage points added to the target for this book (taken off its
    /// reading coverage under a fixed number), from its writing style
    /// ([`crate::style::target_offset`]); 0 by default.
    pub offset: f64,
    /// Where the reader is in it, as a running word, and when it got there
    /// (Unix time): a book started is read first ([`Library::set_reading`]).
    pub reading: Option<(u64, u64)>,
    data: Arc<BookData>,
}

impl std::ops::Deref for LibBook {
    type Target = BookData;
    fn deref(&self) -> &BookData {
        &self.data
    }
}

/// A book's words, numbered across the library.
#[derive(Clone, Debug, PartialEq)]
pub struct BookData {
    /// (word, count), most frequent first.
    words: Vec<(u32, u32)>,
    /// Parallel to `words`: the word contains a name.
    name: Vec<bool>,
    /// Running words that are not names: what shares are taken of.
    denom: u64,
    /// The book's own spelling of a word, where it differs from the key.
    spell: FxHashMap<u32, String>,
    /// Its words as the profile counts them (each spelling apart, hapax
    /// included unless left out), most frequent first: what ✔ Mark read
    /// takes from and what meetings across books count.
    raw: Vec<(u32, u32)>,
    /// Where its words occur, if its profile has them.
    pos: Option<BookPos>,
    /// Its words in the order they were first met in its profile: the order
    /// that numbers them, so that a library built again with this book kept
    /// ([`Source::Kept`]) numbers every word as one built from the files.
    first: Vec<u32>,
}

impl BookData {
    /// The same words, renumbered by `to` (parallel to `first`), in the order
    /// a book read with those numbers has them.
    fn renumbered(&self, to: &[u32]) -> BookData {
        let map: FxHashMap<u32, u32> = self.first.iter().copied().zip(to.iter().copied()).collect();
        let at = |w: u32| map[&w];
        // Ties in count go by number (as when read), so the order may change.
        let mut words: Vec<(usize, u32, u32)> = self
            .words
            .iter()
            .enumerate()
            .map(|(i, &(w, n))| (i, at(w), n))
            .collect();
        words.sort_unstable_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
        BookData {
            name: words.iter().map(|w| self.name[w.0]).collect(),
            pos: self.pos.as_ref().map(|p| BookPos {
                sections: p.sections.clone(),
                total: p.total,
                words: words.iter().map(|w| p.words[w.0].clone()).collect(),
            }),
            words: words.iter().map(|w| (w.1, w.2)).collect(),
            denom: self.denom,
            spell: self
                .spell
                .iter()
                .map(|(w, s)| (at(*w), s.clone()))
                .collect(),
            raw: self.raw.iter().map(|&(w, n)| (at(w), n)).collect(),
            first: to.to_vec(),
        }
    }
}

/// A book for [`Library::rebuild`].
pub enum Source {
    /// Read from its profile, with where its words occur when known.
    Read(Profile, Option<crate::profile::Positions>),
    /// The book at this index in the earlier library, as it was read there.
    Kept(usize),
}

/// The library's words, numbered as first met.
#[derive(Default)]
struct Numbering {
    ids: FxHashMap<String, u32>,
    vocab: Vec<String>,
}

impl Numbering {
    /// The number of `key` (already folded).
    fn key(&mut self, key: &str) -> u32 {
        if let Some(&i) = self.ids.get(key) {
            return i;
        }
        let i = self.vocab.len() as u32;
        self.vocab.push(key.to_owned());
        self.ids.insert(key.to_owned(), i);
        i
    }
}

/// [`crate::profile::Positions`] of a book, by the library's word numbers.
#[derive(Clone, Debug, PartialEq)]
struct BookPos {
    sections: Vec<crate::profile::PosSection>,
    total: u64,
    /// Parallel to [`LibBook::words`]: (section, count), in order.
    words: Vec<Vec<(u32, u32)>>,
}

impl LibBook {
    pub fn unique_words(&self) -> usize {
        self.words.len()
    }
    pub fn running_words(&self) -> u64 {
        self.words.iter().map(|w| w.1 as u64).sum()
    }
    /// The running word `fraction` of the way through its EPUB file `href`
    /// (where the reader is), if it knows where its words occur.
    pub fn running_word_at(&self, href: &str, fraction: f64) -> Option<u64> {
        crate::profile::running_word_in(&self.pos.as_ref()?.sections, href, fraction)
    }
    /// Whether it knows where its words occur.
    pub fn has_positions(&self) -> bool {
        self.pos.is_some()
    }
    /// Its running words, as positions count them (names included).
    fn span(&self) -> u64 {
        self.pos
            .as_ref()
            .map_or_else(|| self.running_words(), |p| p.total)
    }
    /// Expected occurrences of its `i`-th word in running words `a..b`:
    /// even within each section, and past the sections kept, even over the
    /// rest of the book; without positions, even over the whole book.
    fn occurs(&self, i: usize, a: u64, b: u64) -> f64 {
        if b <= a {
            return 0.0;
        }
        let n = self.words[i].1 as f64;
        let overlap =
            |s: u64, e: u64| (b.min(e) as f64 - a.max(s) as f64).max(0.0) / (e - s).max(1) as f64;
        let Some(p) = &self.pos else {
            let span = self.span();
            return if span == 0 { 0.0 } else { n * overlap(0, span) };
        };
        let mut got = 0.0;
        let mut kept = 0.0;
        let mut end = 0;
        for &(si, c) in &p.words[i] {
            let sec = &p.sections[si as usize];
            let e = sec.start + sec.words;
            got += c as f64 * overlap(sec.start, e);
            kept += c as f64;
            end = e;
        }
        let rest = n - kept;
        if rest > 0.0 && end < p.total {
            got += rest * overlap(end, p.total);
        }
        got
    }
}

/// The books, with every word numbered once (after Traditional →
/// Simplified folding when `fold`).
#[derive(Clone, Debug)]
pub struct Library {
    pub fold: bool,
    vocab: Arc<Vec<String>>,
    pub books: Vec<LibBook>,
}

/// A book scanned against a set of known words.
struct Scan {
    known: u64,
    total: u64,
    /// Unknown words that are not names, most frequent first.
    unknown: Vec<(u32, u32)>,
}

impl Scan {
    fn percent(&self) -> f64 {
        if self.total == 0 {
            100.0
        } else {
            self.known as f64 * 100.0 / self.total as f64
        }
    }
}

/// One book on the path.
#[derive(Clone, Debug)]
pub struct Step {
    /// Index into [`Library::books`].
    pub book: usize,
    /// Share of running words known now, before any of the path.
    pub now: f64,
    /// Known when the path reaches it, before studying.
    pub reached: f64,
    /// Known while reading it, after studying.
    pub reading: f64,
    /// The words to study, in order of value.
    pub study: Vec<u32>,
    /// Under a target: the fewest words that would reach it if this book
    /// were read first.
    pub study_now: Option<usize>,
    /// The book's words not known now that the path taught before it.
    pub gained: usize,
    /// The book's [`LibBook::offset`].
    pub offset: f64,
    /// Words learned from reading it ([`Params::incidental`], or learning
    /// while reading: `learned.len()`).
    pub by_reading: usize,
    /// Learning while reading: running words known over its first
    /// [`OPENING`] running words (from where the reader is, in a book
    /// started), as its words are learned; `reading` is the same over the
    /// whole book (the rest of it).
    pub start: f64,
    /// Learning while reading: unknown words met reading it (the rest of it).
    pub lookups: u64,
    /// Learning while reading: the words it teaches, most frequent first.
    pub learned: Vec<u32>,
    /// A book started: how far the reader is, as a share of it.
    pub from: Option<f64>,
    /// A book started: of `learned`, those met often enough already.
    pub learned_so_far: usize,
}

#[derive(Clone, Debug)]
pub struct Path {
    pub steps: Vec<Step>,
    pub params: Params,
}

impl Path {
    /// Words studied over the whole path.
    pub fn total_study(&self) -> usize {
        self.steps.iter().map(|s| s.study.len()).sum()
    }
    /// The largest number of words studied before one book.
    pub fn peak(&self) -> usize {
        self.steps.iter().map(|s| s.study.len()).max().unwrap_or(0)
    }
    /// Learning while reading: the lowest share of running words known at
    /// the start of a book.
    pub fn lowest_start(&self) -> f64 {
        self.steps
            .iter()
            .map(|s| s.start)
            .fold(f64::INFINITY, f64::min)
    }
    /// Learning while reading: unknown words met over the whole path.
    pub fn total_lookups(&self) -> u64 {
        self.steps.iter().map(|s| s.lookups).sum()
    }
    /// Learning while reading: words the path teaches.
    pub fn total_learned(&self) -> usize {
        self.steps.iter().map(|s| s.learned.len()).sum()
    }
    /// The lowest share of running words known while reading a book.
    pub fn lowest_reading(&self) -> f64 {
        self.steps
            .iter()
            .map(|s| s.reading)
            .fold(f64::INFINITY, f64::min)
    }
    pub fn order(&self) -> Vec<usize> {
        self.steps.iter().map(|s| s.book).collect()
    }
    /// The largest step cost and their sum: smaller is better (see the
    /// module documentation).
    pub fn key(&self) -> Key {
        order_key(
            self.params.objective,
            self.steps
                .iter()
                .fold(EMPTY, |k, s| add_cost(k, step_cost(self.params.study, s))),
        )
    }
}

/// Two step-cost aggregates, compared in order: smaller is better.
pub type Key = [i64; 2];

/// (largest step cost, sum of step costs), as accumulated.
const EMPTY: Key = [i64::MIN, 0];

/// A step's part of the largest and of the sum.
type Cost = (i64, i64);

fn add_cost(k: Key, c: Cost) -> Key {
    [k[0].max(c.0), k[1] + c.1]
}

/// The accumulated (largest, sum) in the order `objective` compares them.
fn order_key(objective: Objective, k: Key) -> Key {
    match objective {
        Objective::Peak => k,
        Objective::Total => [k[1], k[0]],
    }
}

/// Words studied, or under a fixed number, the reading coverage in
/// hundredths of a percent, negated (so that smaller is better); learning
/// while reading, the coverage at the start so, and the unknown words met.
fn step_cost(study: Study, s: &Step) -> Cost {
    match study {
        Study::Target(_) => (s.study.len() as i64, s.study.len() as i64),
        Study::Fixed(_) => {
            let c = -((s.reading - s.offset) * 100.0).round() as i64;
            (c, c)
        }
        Study::Read { .. } => (
            -((s.start - s.offset) * 100.0).round() as i64,
            s.lookups as i64,
        ),
    }
}

/// What reading a book teaches and what it is like, learning while reading.
struct ReadEval {
    start: f64,
    over: f64,
    lookups: f64,
    learned: Vec<u32>,
    so_far: usize,
}

/// Precomputed per-word invariants for [`Study::Read`].
struct ReadWord {
    w: u32,
    n: u32,
    in_sel: bool,
    name: bool,
    before: f64,
    here: f64,
    rest: f64,
}

/// Precomputed per-book invariants for [`Study::Read`] (`selection` and `occurs`).
struct BookReadCache {
    span: u64,
    from: u64,
    win: u64,
    words: Vec<ReadWord>,
}

/// A partial path in the search.
#[derive(Clone)]
struct State {
    order: Vec<usize>,
    known: Known,
    /// Each word's share summed over the books still to come (lookahead only).
    future: Vec<f32>,
    /// Books on the path, as a bit set over the library.
    done: Vec<u64>,
    key: Key,
}

impl Library {
    /// `books`: (title, profile). `fold` must be the
    /// knowledge's (see [`Library::fold_for`]). `skip_hapax`: leave out a
    /// book's words that occur only once in it, as the Overview does.
    pub fn build(fold: bool, skip_hapax: bool, books: Vec<(String, Profile)>) -> Library {
        Self::build_with(
            fold,
            skip_hapax,
            books.into_iter().map(|(t, p)| (t, p, None)).collect(),
        )
    }

    /// [`Library::build`], with where each book's words occur, where known
    /// ([`crate::profile::Positions`] made with that profile).
    pub fn build_with(
        fold: bool,
        skip_hapax: bool,
        books: Vec<(String, Profile, Option<crate::profile::Positions>)>,
    ) -> Library {
        Self::rebuild(
            fold,
            skip_hapax,
            books
                .into_iter()
                .map(|(t, p, pos)| (t, Source::Read(p, pos)))
                .collect(),
            None,
        )
    }

    /// [`Library::build_with`], with books that have not changed since
    /// `old` was built taken from it ([`Source::Kept`]) instead of read
    /// again. `old` must have been built with the same `fold` and
    /// `skip_hapax`. The library is the same as one built from the files.
    pub fn rebuild(
        fold: bool,
        skip_hapax: bool,
        books: Vec<(String, Source)>,
        old: Option<&Library>,
    ) -> Library {
        debug_assert!(old.is_none_or(|o| o.fold == fold));
        let mut num = Numbering::default();
        let mut out = Vec::new();
        for (title, src) in books {
            match src {
                Source::Read(p, positions) => {
                    out.push(Self::read_book(
                        title, p, positions, fold, skip_hapax, &mut num,
                    ));
                }
                Source::Kept(i) => {
                    let old = old.expect("a kept book needs the library it is kept from");
                    let o = &old.books[i];
                    // Numbered as if read now: the same numbers unless an
                    // earlier book changed.
                    let to: Vec<u32> = o
                        .first
                        .iter()
                        .map(|&w| num.key(&old.vocab[w as usize]))
                        .collect();
                    let data = if to == o.first {
                        o.data.clone()
                    } else {
                        Arc::new(o.data.renumbered(&to))
                    };
                    out.push(LibBook {
                        id: o.id.clone(),
                        title,
                        segmenter: o.segmenter.clone(),
                        #[cfg(test)]
                        variant: o.variant.clone(),
                        series: None,
                        offset: 0.0,
                        reading: None,
                        data,
                    });
                }
            }
        }
        // Series: two or more books whose titles name one, at least one with
        // a volume number ([`Library::set_series`] adds the books' metadata).
        let vols: Vec<series::Volume> = out.iter().map(|b| series::parse(&b.title)).collect();
        for (b, s) in out.iter_mut().zip(series::group(&vols)) {
            b.series = s;
        }
        Library {
            fold,
            vocab: Arc::new(num.vocab),
            books: out,
        }
    }

    /// One book read from its profile, its words numbered by `num`.
    fn read_book(
        title: String,
        p: Profile,
        positions: Option<crate::profile::Positions>,
        fold: bool,
        skip_hapax: bool,
        num: &mut Numbering,
    ) -> LibBook {
        // Single characters would match far too much (as on the Overview).
        let names: Vec<&str> = p
            .names
            .iter()
            .filter(|n| n.chars().count() >= 2)
            .map(String::as_str)
            .collect();
        let ac = (!names.is_empty())
            .then(|| aho_corasick::AhoCorasick::new(&names).ok())
            .flatten();
        let mut counts: FxHashMap<u32, (u32, bool)> = FxHashMap::default();
        let mut first: Vec<u32> = Vec::new();
        let mut spell: FxHashMap<u32, String> = FxHashMap::default();
        let mut raw: Vec<(u32, u32)> = Vec::new();
        let mut placed: FxHashMap<u32, Vec<(u32, u32)>> = FxHashMap::default();
        // Before folding, so the same words go as on the Overview.
        for (pi, (w, n)) in p
            .words
            .iter()
            .enumerate()
            .filter(|(_, (_, n))| !skip_hapax || *n > 1)
        {
            let k = crate::anki::fold(w, fold);
            let unfolded = k == *w;
            let id = num.key(&k);
            raw.push((id, *n));
            if let Some(pos) = &positions {
                placed.entry(id).or_default().extend(&pos.words[pi]);
            }
            let is_name = ac.as_ref().is_some_and(|a| a.is_match(w.as_str()));
            let e = counts.entry(id).or_insert_with(|| {
                first.push(id);
                (0, false)
            });
            e.0 += n;
            e.1 |= is_name;
            // The book's most frequent spelling of a folded word is how it is shown.
            if !unfolded && !spell.contains_key(&id) {
                spell.insert(id, w.clone());
            }
        }
        let mut words: Vec<(u32, u32, bool)> =
            counts.into_iter().map(|(w, (n, nm))| (w, n, nm)).collect();
        words.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let denom = words.iter().filter(|w| !w.2).map(|w| w.1 as u64).sum();
        // Stable: equal counts keep the profile's order, as Mark read.
        raw.sort_by_key(|x| std::cmp::Reverse(x.1));
        let pos = positions.map(|ps| BookPos {
            total: ps.total(),
            words: words
                .iter()
                .map(|w| {
                    // Spellings folded into one word: their sections merged.
                    let mut v = placed.remove(&w.0).unwrap_or_default();
                    v.sort_unstable_by_key(|x| x.0);
                    v.dedup_by(|b, a| {
                        let same = a.0 == b.0;
                        if same {
                            a.1 += b.1;
                        }
                        same
                    });
                    v
                })
                .collect(),
            sections: ps.sections,
        });
        LibBook {
            id: p.id,
            title,
            segmenter: p.segmenter,
            #[cfg(test)]
            variant: p.variant,
            series: None,
            offset: 0.0,
            reading: None,
            data: Arc::new(BookData {
                name: words.iter().map(|w| w.2).collect(),
                words: words.iter().map(|w| (w.0, w.1)).collect(),
                denom,
                spell,
                raw,
                pos,
                first,
            }),
        }
    }

    /// The folding a library must be built with to match `k`.
    pub fn fold_for(k: Option<&Knowledge>, match_across_scripts: bool) -> bool {
        k.map_or(match_across_scripts, |k| k.fold)
    }

    /// The library's words that `k` knows.
    pub fn known(&self, k: Option<&Knowledge>) -> Known {
        let mut out = Known::new(self.vocab.len());
        if let Some(k) = k {
            for (i, w) in self.vocab.iter().enumerate() {
                if k.word(w).is_known() {
                    out.add(i as u32);
                }
            }
        }
        out
    }

    /// Add to `k` the meetings of the books read (their word counts, as
    /// [`crate::known::across_books`] takes them): learning while reading
    /// counts them towards a word's [`Study::Read`] `min_count`.
    pub fn add_read_meetings<'a>(
        &self,
        k: &mut Known,
        books: impl IntoIterator<Item = &'a [(String, u32)]>,
    ) {
        let index: FxHashMap<&str, u32> = self
            .vocab
            .iter()
            .enumerate()
            .map(|(i, w)| (w.as_str(), i as u32))
            .collect();
        for words in books {
            for (w, n) in words {
                if let Some(&i) = index.get(crate::anki::fold(w, self.fold).as_str()) {
                    k.add_met(i, *n);
                }
            }
        }
    }

    /// Where the reader is in each book: (running word, when); a book is
    /// started when that is past its first 1%. True if one changed.
    pub fn set_reading(&mut self, at: impl Fn(&LibBook) -> Option<(u64, u64)>) -> bool {
        let mut changed = false;
        for i in 0..self.books.len() {
            let r = Self::started_at(&self.books[i], &at);
            changed |= r != self.books[i].reading;
            self.books[i].reading = r;
        }
        changed
    }

    /// Whether [`Library::set_reading`] with `at` would change anything.
    pub fn reading_differs(&self, at: impl Fn(&LibBook) -> Option<(u64, u64)>) -> bool {
        self.books
            .iter()
            .any(|b| Self::started_at(b, &at) != b.reading)
    }

    fn started_at(b: &LibBook, at: &impl Fn(&LibBook) -> Option<(u64, u64)>) -> Option<(u64, u64)> {
        let span = b.span();
        at(b).filter(|r| span > 0 && r.0 * 100 >= span)
    }

    /// Learning while reading, the books of `books` started, most recently
    /// read first: they come first on every path.
    pub fn started(&self, params: &Params, books: &[usize]) -> Vec<usize> {
        if !matches!(params.study, Study::Read { .. }) {
            return Vec::new();
        }
        let mut v: Vec<usize> = books
            .iter()
            .copied()
            .filter(|&b| self.books[b].reading.is_some())
            .collect();
        v.sort_by_key(|&b| (std::cmp::Reverse(self.books[b].reading.unwrap().1), b));
        v
    }

    /// The books every path of `books` starts with: those started
    /// ([`Library::started`]), and keeping series together, each followed
    /// by the rest of its series: its later volumes in order, then any
    /// earlier ones (not marked read) in order.
    pub fn first_books(&self, params: &Params, books: &[usize]) -> Vec<usize> {
        let started = self.started(params, books);
        if !params.keep_series {
            return started;
        }
        let mut out: Vec<usize> = Vec::new();
        for b in started {
            if out.contains(&b) {
                continue;
            }
            out.push(b);
            if let Some((s, _)) = &self.books[b].series {
                let (before, after): (Vec<usize>, Vec<usize>) = self
                    .volumes(*s, books)
                    .into_iter()
                    .filter(|&v| v != b)
                    .partition(|&v| self.precedes(v, b));
                for v in after.into_iter().chain(before) {
                    if !out.contains(&v) {
                        out.push(v);
                    }
                }
            }
        }
        out
    }

    /// `order` with the books every path starts with
    /// ([`Library::first_books`]) moved to the front, and keeping series
    /// together, each series where its first book in `order` is.
    pub fn started_first(&self, params: &Params, order: &[usize]) -> Vec<usize> {
        let order = if params.keep_series {
            self.series_together(order)
        } else {
            order.to_vec()
        };
        let first = self.first_books(params, &order);
        let rest = order.iter().copied().filter(|b| !first.contains(b));
        first.iter().copied().chain(rest).collect()
    }

    /// Distinct words across the library.
    pub fn vocab_len(&self) -> usize {
        self.vocab.len()
    }

    pub fn word(&self, id: u32) -> &str {
        &self.vocab[id as usize]
    }

    /// Study words as `book` spells them, with their counts there.
    pub fn study_list(&self, book: usize, words: &[u32]) -> Vec<(String, u32)> {
        let b = &self.books[book];
        let counts: FxHashMap<u32, u32> = b.words.iter().copied().collect();
        words
            .iter()
            .map(|w| {
                let shown = b
                    .spell
                    .get(w)
                    .cloned()
                    .unwrap_or_else(|| self.word(*w).to_owned());
                (shown, counts.get(w).copied().unwrap_or(0))
            })
            .collect()
    }

    /// `a` is an earlier volume of `b`'s series, so it is read before `b`.
    pub fn precedes(&self, a: usize, b: usize) -> bool {
        match (&self.books[a].series, &self.books[b].series) {
            (Some((sa, va)), Some((sb, vb))) => sa == sb && va < vb,
            _ => false,
        }
    }

    /// `order` with the books of each series moved into volume order, in the
    /// places that series has in it; other books stay where they are.
    pub fn in_series_order(&self, order: &[usize]) -> Vec<usize> {
        let mut places: FxHashMap<u32, Vec<usize>> = FxHashMap::default();
        for (i, &b) in order.iter().enumerate() {
            if let Some((s, _)) = &self.books[b].series {
                places.entry(*s).or_default().push(i);
            }
        }
        let mut out = order.to_vec();
        let vol = |b: usize| &self.books[b].series.as_ref().unwrap().1;
        for at in places.values() {
            let mut bs: Vec<usize> = at.iter().map(|&i| order[i]).collect();
            // Stable: volumes that compare equal keep their order.
            bs.sort_by(|&x, &y| vol(x).cmp(vol(y)));
            for (&i, b) in at.iter().zip(bs) {
                out[i] = b;
            }
        }
        out
    }

    /// `a` and `b` are books of one series.
    pub fn same_series(&self, a: usize, b: usize) -> bool {
        match (&self.books[a].series, &self.books[b].series) {
            (Some((sa, _)), Some((sb, _))) => sa == sb,
            _ => false,
        }
    }

    /// The books of `books` in series `s`, in volume order (volumes that
    /// compare equal in their order in `books`).
    fn volumes(&self, s: u32, books: &[usize]) -> Vec<usize> {
        let vol = |b: usize| {
            self.books[b]
                .series
                .as_ref()
                .filter(|x| x.0 == s)
                .map(|x| &x.1)
        };
        let mut v: Vec<usize> = books
            .iter()
            .copied()
            .filter(|&b| vol(b).is_some())
            .collect();
        v.sort_by(|&x, &y| vol(x).cmp(&vol(y)));
        v
    }

    /// `order` with each series' books moved together, in volume order, to
    /// where the first of them is; other books keep their order.
    pub fn series_together(&self, order: &[usize]) -> Vec<usize> {
        let mut out = Vec::with_capacity(order.len());
        let mut placed: FxHashSet<u32> = FxHashSet::default();
        for &b in order {
            match &self.books[b].series {
                None => out.push(b),
                Some((s, _)) => {
                    if placed.insert(*s) {
                        out.extend(self.volumes(*s, order));
                    }
                }
            }
        }
        out
    }

    /// Keeping series together, the series of `books` with two or more
    /// books there, each in volume order.
    fn blocks(&self, params: &Params, books: &[usize]) -> Vec<Vec<usize>> {
        if !params.keep_series {
            return Vec::new();
        }
        let mut ids: Vec<u32> = books
            .iter()
            .filter_map(|&b| self.books[b].series.as_ref().map(|s| s.0))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter()
            .map(|s| self.volumes(s, books))
            .filter(|v| v.len() > 1)
            .collect()
    }

    /// For each book of the library, the books of `books` that precede it.
    fn earlier_volumes(&self, books: &[usize]) -> Vec<Vec<usize>> {
        let mut v = vec![Vec::new(); self.books.len()];
        for &b in books {
            v[b] = books
                .iter()
                .copied()
                .filter(|&a| self.precedes(a, b))
                .collect();
        }
        v
    }

    /// Distinct segmenters among `books`.
    pub fn segmenters(&self, books: &[usize]) -> Vec<&str> {
        let mut v: Vec<&str> = books
            .iter()
            .map(|&b| self.books[b].segmenter.as_str())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    fn scan(&self, book: usize, k: &Known) -> Scan {
        let b = &self.books[book];
        let mut s = Scan {
            known: 0,
            total: 0,
            unknown: Vec::new(),
        };
        for (&(w, n), &name) in b.words.iter().zip(&b.name) {
            if k.has(w) {
                s.known += n as u64;
                s.total += n as u64;
            } else if !name {
                s.total += n as u64;
                s.unknown.push((w, n));
            }
        }
        s
    }

    /// Share known now, and the cheap cost of studying `book` next: the
    /// fewest words reaching the target, or (fixed) the reading coverage
    /// after its most frequent unknown words, without building the list.
    fn cheap(&self, book: usize, k: &Known, study: Study) -> (f64, Cost) {
        let b = &self.books[book];
        let (mut known, mut total) = (0u64, 0u64);
        for (&(w, n), &name) in b.words.iter().zip(&b.name) {
            if k.has(w) {
                known += n as u64;
                total += n as u64;
            } else if !name {
                total += n as u64;
            }
        }
        let pct = |x: u64| {
            if total == 0 {
                100.0
            } else {
                x as f64 * 100.0 / total as f64
            }
        };
        let now = pct(known);
        // Unknown non-name words in frequency order, lazily.
        let unknown = b
            .words
            .iter()
            .zip(&b.name)
            .filter(|(w, name)| !**name && !k.has(w.0))
            .map(|(w, _)| w.1 as u64);
        match study {
            Study::Target(t) => {
                let need = need_tokens((t + b.offset).clamp(0.0, 100.0), known, total);
                let mut got = 0;
                let mut n = 0;
                for c in unknown {
                    if got >= need {
                        break;
                    }
                    got += c;
                    n += 1;
                }
                (now, (n, n))
            }
            Study::Fixed(n) => {
                let add: u64 = unknown.take(n).sum();
                let c = -((pct(known + add) - b.offset) * 100.0).round() as i64;
                (now, (c, c))
            }
            Study::Read { percent, min_count } => {
                let rc = self.book_read_cache(book, percent, min_count);
                let e = Self::read_eval_cached(k, min_count, &rc, false);
                (
                    now,
                    (
                        -((e.start - b.offset) * 100.0).round() as i64,
                        e.lookups.round() as i64,
                    ),
                )
            }
        }
    }

    /// Cheap step cost for `book` next from `k`, using precomputed `read_cache`
    /// under [`Study::Read`] without scanning `b.words` twice.
    fn cheap_cost(
        &self,
        book: usize,
        k: &Known,
        study: Study,
        read_cache: Option<&[Option<BookReadCache>]>,
    ) -> Cost {
        let b = &self.books[book];
        if let Study::Read { percent, min_count } = study {
            let owned;
            let rc = match read_cache.and_then(|c| c[book].as_ref()) {
                Some(rc) => rc,
                None => {
                    owned = self.book_read_cache(book, percent, min_count);
                    &owned
                }
            };
            let e = Self::read_eval_cached(k, min_count, rc, false);
            return (
                -((e.start - b.offset) * 100.0).round() as i64,
                e.lookups.round() as i64,
            );
        }
        self.cheap(book, k, study).1
    }

    /// The words of `book` that ✔ Mark read takes ([`crate::known::read_selection`]):
    /// of those it uses at least `min_count` times, the `percent`% most frequent.
    fn selection(&self, book: usize, percent: u8, min_count: u32) -> FxHashSet<u32> {
        let raw = &self.books[book].raw;
        // Most frequent first, so those used often enough come first.
        let often = raw.partition_point(|x| x.1 >= min_count);
        let take = (often * percent.min(100) as usize).div_ceil(100);
        raw[..take].iter().map(|x| x.0).collect()
    }

    /// Precompute per-word `selection` and `occurs` invariants for `book`.
    fn book_read_cache(&self, book: usize, percent: u8, min_count: u32) -> BookReadCache {
        let b = &self.books[book];
        let sel = self.selection(book, percent, min_count);
        let span = b.span();
        let from = b.reading.map_or(0, |r| r.0.min(span));
        let win = (from + OPENING).min(span);
        let words = b
            .words
            .iter()
            .zip(&b.name)
            .enumerate()
            .map(|(i, (&(w, n), &name))| {
                let (before, here, rest) = if name {
                    (0.0, 0.0, 0.0)
                } else {
                    let before = if from > 0 { b.occurs(i, 0, from) } else { 0.0 };
                    let here = b.occurs(i, from, win);
                    let rest = (n as f64 - before).max(0.0);
                    (before, here, rest)
                };
                ReadWord {
                    w,
                    n,
                    in_sel: sel.contains(&w),
                    name,
                    before,
                    here,
                    rest,
                }
            })
            .collect();
        BookReadCache {
            span,
            from,
            win,
            words,
        }
    }

    fn build_read_cache(
        &self,
        books: &[usize],
        params: &Params,
    ) -> Option<Vec<Option<BookReadCache>>> {
        let Study::Read { percent, min_count } = params.study else {
            return None;
        };
        let mut cache: Vec<Option<BookReadCache>> = (0..self.books.len()).map(|_| None).collect();
        let built: Vec<(usize, BookReadCache)> = books
            .par_iter()
            .copied()
            .map(|book| (book, self.book_read_cache(book, percent, min_count)))
            .collect();
        for (book, rc) in built {
            cache[book] = Some(rc);
        }
        Some(cache)
    }

    /// Learning while reading `book` from `k` using precomputed `rc`.
    fn read_eval_cached(k: &Known, min_count: u32, rc: &BookReadCache, list: bool) -> ReadEval {
        // Running words, names left out unless known, as the Overview counts them.
        let mut total = 0u64;
        let (mut lk_win, mut lk_rest) = (0f64, 0f64);
        let mut learned = Vec::new();
        let mut so_far = 0;
        for rw in &rc.words {
            if k.has(rw.w) {
                total += rw.n as u64;
                continue;
            }
            // Met this many times more, it is learned (`None`: not in this book).
            let e = k.met(rw.w);
            let after = if rw.in_sel || (e > 0 && e + rw.n >= min_count) {
                Some(min_count.saturating_sub(e) as f64)
            } else {
                None
            };
            if after.is_some() && list {
                learned.push(rw.w);
            }
            if rw.name {
                continue;
            }
            total += rw.n as u64;
            match after {
                Some(a) => {
                    let left = (a - rw.before).max(0.0);
                    lk_win += left.min(rw.here);
                    lk_rest += left.min(rw.rest);
                    if rc.from > 0 && rw.before >= a {
                        so_far += 1;
                    }
                }
                None => {
                    lk_win += rw.here;
                    lk_rest += rw.rest;
                }
            }
        }
        // Running words over the stretches, names left out as in `total`.
        let share = |a: u64, z: u64| {
            if rc.span == 0 {
                0.0
            } else {
                total as f64 * (z - a) as f64 / rc.span as f64
            }
        };
        let pct = |lk: f64, toks: f64| {
            if toks <= 0.0 {
                100.0
            } else {
                (100.0 * (1.0 - lk / toks)).clamp(0.0, 100.0)
            }
        };
        ReadEval {
            start: pct(lk_win, share(rc.from, rc.win)),
            over: pct(lk_rest, share(rc.from, rc.span)),
            lookups: lk_rest,
            learned,
            so_far,
        }
    }

    /// Advance `k` and `future` by reading `book` and return only its step cost,
    /// without constructing a full [`Step`].
    fn step_cost_only(
        &self,
        book: usize,
        k: &mut Known,
        future: &mut [f32],
        params: &Params,
        read_cache: Option<&[Option<BookReadCache>]>,
    ) -> Cost {
        let b = &self.books[book];
        if let Study::Read { percent, min_count } = params.study {
            let owned;
            let rc = match read_cache.and_then(|c| c[book].as_ref()) {
                Some(rc) => rc,
                None => {
                    owned = self.book_read_cache(book, percent, min_count);
                    &owned
                }
            };
            let mut total = 0u64;
            let (mut lk_win, mut lk_rest) = (0f64, 0f64);
            for rw in &rc.words {
                if k.has(rw.w) {
                    total += rw.n as u64;
                    continue;
                }
                let e = k.met(rw.w);
                let after = if rw.in_sel || (e > 0 && e + rw.n >= min_count) {
                    Some(min_count.saturating_sub(e) as f64)
                } else {
                    None
                };
                if after.is_some() {
                    k.add(rw.w);
                }
                if rw.name {
                    continue;
                }
                total += rw.n as u64;
                match after {
                    Some(a) => {
                        let left = (a - rw.before).max(0.0);
                        lk_win += left.min(rw.here);
                        lk_rest += left.min(rw.rest);
                    }
                    None => {
                        lk_win += rw.here;
                        lk_rest += rw.rest;
                    }
                }
            }
            for &(w, n) in &b.raw {
                k.add_met(w, n);
            }
            let toks_win = if rc.span == 0 {
                0.0
            } else {
                total as f64 * (rc.win - rc.from) as f64 / rc.span as f64
            };
            let start = if toks_win <= 0.0 {
                100.0
            } else {
                (100.0 * (1.0 - lk_win / toks_win)).clamp(0.0, 100.0)
            };
            return (
                -((start - b.offset) * 100.0).round() as i64,
                lk_rest.round() as u64 as i64,
            );
        }

        let lam = params.lookahead;
        let use_lookahead = lam > 0.0 && !future.is_empty() && b.denom > 0;
        let (mut known, mut total) = (0u64, 0u64);
        let cost = if use_lookahead {
            let d = b.denom as f32;
            let mut v: Vec<(f32, u32, u32)> = Vec::new();
            for (&(w, n), &name) in b.words.iter().zip(&b.name) {
                if k.has(w) {
                    known += n as u64;
                    total += n as u64;
                } else if !name {
                    total += n as u64;
                    let own = n as f32 / d;
                    let val = own + lam * (future[w as usize] - own).max(0.0);
                    v.push((val, w, n));
                }
            }
            v.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(b.2.cmp(&a.2)).then(a.1.cmp(&b.1)));
            let mut studied = 0usize;
            let mut add = 0u64;
            match params.study {
                Study::Target(t) => {
                    let need = need_tokens((t + b.offset).clamp(0.0, 100.0), known, total);
                    for &(_, w, n) in &v {
                        if add >= need {
                            break;
                        }
                        k.add(w);
                        add += n as u64;
                        studied += 1;
                    }
                }
                Study::Fixed(n_study) => {
                    for &(_, w, c) in v.iter().take(n_study) {
                        k.add(w);
                        add += c as u64;
                        studied += 1;
                    }
                }
                Study::Read { .. } => {}
            }
            if let Some(m) = params.incidental {
                for &(_, w, n) in &v {
                    if n >= m && !k.has(w) {
                        k.add(w);
                    }
                }
            }
            match params.study {
                Study::Target(_) => (studied as i64, studied as i64),
                Study::Fixed(_) => {
                    let reading = if total == 0 {
                        100.0
                    } else {
                        (known + add) as f64 * 100.0 / total as f64
                    };
                    let c = -((reading - b.offset) * 100.0).round() as i64;
                    (c, c)
                }
                Study::Read { .. } => (0, 0),
            }
        } else {
            for (&(w, n), &name) in b.words.iter().zip(&b.name) {
                if k.has(w) {
                    known += n as u64;
                    total += n as u64;
                } else if !name {
                    total += n as u64;
                }
            }
            let mut studied = 0usize;
            let mut add = 0u64;
            match params.study {
                Study::Target(t) => {
                    let need = need_tokens((t + b.offset).clamp(0.0, 100.0), known, total);
                    for (&(w, n), &name) in b.words.iter().zip(&b.name) {
                        if name || k.has(w) {
                            continue;
                        }
                        if add < need {
                            k.add(w);
                            add += n as u64;
                            studied += 1;
                        } else if let Some(m) = params.incidental {
                            if n >= m {
                                k.add(w);
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
                Study::Fixed(n_study) => {
                    for (&(w, c), &name) in b.words.iter().zip(&b.name) {
                        if name || k.has(w) {
                            continue;
                        }
                        if studied < n_study {
                            k.add(w);
                            add += c as u64;
                            studied += 1;
                        } else if let Some(m) = params.incidental {
                            if c >= m {
                                k.add(w);
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
                Study::Read { .. } => {}
            }
            match params.study {
                Study::Target(_) => (studied as i64, studied as i64),
                Study::Fixed(_) => {
                    let reading = if total == 0 {
                        100.0
                    } else {
                        (known + add) as f64 * 100.0 / total as f64
                    };
                    let c = -((reading - b.offset) * 100.0).round() as i64;
                    (c, c)
                }
                Study::Read { .. } => (0, 0),
            }
        };
        // This book is no longer to come.
        if !future.is_empty() && b.denom > 0 {
            let d = b.denom as f32;
            for (&(w, n), &name) in b.words.iter().zip(&b.name) {
                if !name {
                    future[w as usize] -= n as f32 / d;
                }
            }
        }
        cost
    }

    /// The step for `book` from state `k` (and `future`, with lookahead).
    fn step(
        &self,
        book: usize,
        k: &mut Known,
        future: &mut [f32],
        params: &Params,
        known0: &Known,
        read_cache: Option<&[Option<BookReadCache>]>,
    ) -> Step {
        let b = &self.books[book];
        let (mut now_known, mut now_total, mut gained) = (0u64, 0u64, 0usize);
        for (&(w, n), &name) in b.words.iter().zip(&b.name) {
            if known0.has(w) {
                now_known += n as u64;
                now_total += n as u64;
            } else if !name {
                now_total += n as u64;
                if k.has(w) {
                    gained += 1;
                }
            }
        }
        let now = if now_total == 0 {
            100.0
        } else {
            now_known as f64 * 100.0 / now_total as f64
        };
        if let Study::Read { percent, min_count } = params.study {
            let (mut rk, mut rt) = (0u64, 0u64);
            for (&(w, n), &name) in b.words.iter().zip(&b.name) {
                if k.has(w) {
                    rk += n as u64;
                    rt += n as u64;
                } else if !name {
                    rt += n as u64;
                }
            }
            let reached = if rt == 0 {
                100.0
            } else {
                rk as f64 * 100.0 / rt as f64
            };
            let owned;
            let rc = match read_cache.and_then(|c| c[book].as_ref()) {
                Some(rc) => rc,
                None => {
                    owned = self.book_read_cache(book, percent, min_count);
                    &owned
                }
            };
            let e = Self::read_eval_cached(k, min_count, rc, true);
            for &w in &e.learned {
                k.add(w);
            }
            // Its words were met, for the books after it.
            for &(w, n) in &b.raw {
                k.add_met(w, n);
            }
            let span = rc.span;
            return Step {
                book,
                now,
                reached,
                reading: e.over,
                study: Vec::new(),
                study_now: None,
                gained,
                by_reading: e.learned.len(),
                offset: b.offset,
                start: e.start,
                lookups: e.lookups.round() as u64,
                from: b
                    .reading
                    .filter(|_| span > 0)
                    .map(|r| r.0.min(span) as f64 / span as f64),
                learned_so_far: e.so_far,
                learned: e.learned,
            };
        }
        let s = self.scan(book, k);
        let reached = s.percent();
        // Value order: this book's share plus the books still to come.
        let mut cand = s.unknown;
        let lam = params.lookahead;
        if lam > 0.0 && !future.is_empty() && b.denom > 0 {
            let d = b.denom as f32;
            let value = |&(w, n): &(u32, u32)| {
                let own = n as f32 / d;
                own + lam * (future[w as usize] - own).max(0.0)
            };
            let mut v: Vec<(f32, u32, u32)> = cand.iter().map(|x| (value(x), x.0, x.1)).collect();
            v.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(b.2.cmp(&a.2)).then(a.1.cmp(&b.1)));
            cand = v.into_iter().map(|x| (x.1, x.2)).collect();
        }
        let mut study = Vec::new();
        let mut add = 0u64;
        match params.study {
            Study::Target(t) => {
                let need = need_tokens((t + b.offset).clamp(0.0, 100.0), s.known, s.total);
                for &(w, n) in &cand {
                    if add >= need {
                        break;
                    }
                    study.push(w);
                    add += n as u64;
                }
            }
            Study::Fixed(n) => {
                for &(w, c) in cand.iter().take(n) {
                    study.push(w);
                    add += c as u64;
                }
            }
            // Returned above.
            Study::Read { .. } => {}
        }
        for &w in &study {
            k.add(w);
        }
        let reading = if s.total == 0 {
            100.0
        } else {
            (s.known + add) as f64 * 100.0 / s.total as f64
        };
        let mut by_reading = 0;
        if let Some(m) = params.incidental {
            for &(w, n) in &cand {
                if n >= m && !k.has(w) {
                    k.add(w);
                    by_reading += 1;
                }
            }
        }
        // This book is no longer to come.
        if !future.is_empty() && b.denom > 0 {
            let d = b.denom as f32;
            for (&(w, n), &name) in b.words.iter().zip(&b.name) {
                if !name {
                    future[w as usize] -= n as f32 / d;
                }
            }
        }
        let study_now = match params.study {
            Study::Target(_) => Some(self.cheap(book, known0, params.study).1.0 as usize),
            Study::Fixed(_) | Study::Read { .. } => None,
        };
        Step {
            book,
            now,
            reached,
            reading,
            study,
            study_now,
            gained,
            by_reading,
            offset: b.offset,
            start: reading,
            lookups: 0,
            learned: Vec::new(),
            from: None,
            learned_so_far: 0,
        }
    }

    /// Each word's share summed over `books`.
    fn future(&self, books: &[usize]) -> Vec<f32> {
        let mut f = vec![0f32; self.vocab.len()];
        for &i in books {
            let b = &self.books[i];
            if b.denom == 0 {
                continue;
            }
            let d = b.denom as f32;
            for (&(w, n), &name) in b.words.iter().zip(&b.name) {
                if !name {
                    f[w as usize] += n as f32 / d;
                }
            }
        }
        f
    }

    /// The path that reads `order` as given.
    pub fn evaluate(&self, known0: &Known, params: &Params, order: &[usize]) -> Path {
        self.evaluate_with(known0, params, order, None)
    }

    fn evaluate_with(
        &self,
        known0: &Known,
        params: &Params,
        order: &[usize],
        read_cache: Option<&[Option<BookReadCache>]>,
    ) -> Path {
        let mut k = known0.clone();
        let mut future = if params.lookahead > 0.0 {
            self.future(order)
        } else {
            Vec::new()
        };
        let steps = order
            .iter()
            .map(|&b| self.step(b, &mut k, &mut future, params, known0, read_cache))
            .collect();
        Path {
            steps,
            params: *params,
        }
    }

    /// The series of the books, each from its EPUB metadata (`meta`, by
    /// History id) when it names one, else from its title ([`series::of`]).
    pub fn series_with(
        &self,
        meta: impl Fn(&str) -> Option<series::Meta>,
    ) -> Vec<Option<(u32, Vec<u32>)>> {
        let vols: Vec<series::Volume> = self
            .books
            .iter()
            .map(|b| series::of(&b.title, meta(&b.id).as_ref()))
            .collect();
        series::group(&vols)
    }

    /// Whether [`Library::set_series`] with these series would change anything.
    pub fn series_differ(&self, s: &[Option<(u32, Vec<u32>)>]) -> bool {
        self.books.iter().zip(s).any(|(b, s)| b.series != *s)
    }

    /// Set the books' series, as [`Library::series_with`] gives them.
    pub fn set_series(&mut self, s: Vec<Option<(u32, Vec<u32>)>>) {
        for (b, s) in self.books.iter_mut().zip(s) {
            b.series = s;
        }
    }

    /// Set each book's [`LibBook::offset`] by its History id. True if one changed.
    pub fn set_offsets(&mut self, offset: impl Fn(&str) -> f64) -> bool {
        let mut changed = false;
        for b in &mut self.books {
            let o = offset(&b.id);
            changed |= o != b.offset;
            b.offset = o;
        }
        changed
    }

    /// `books` sorted by the share of running words known now (less any
    /// [`LibBook::offset`]), highest first, with each series in volume order.
    pub fn easiest_now(&self, known0: &Known, books: &[usize]) -> Vec<usize> {
        let mut v: Vec<(usize, f64)> = books
            .iter()
            .map(|&b| {
                (
                    b,
                    self.cheap(b, known0, Study::Fixed(0)).0 - self.books[b].offset,
                )
            })
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let v: Vec<usize> = v.into_iter().map(|x| x.0).collect();
        self.in_series_order(&v)
    }

    /// The best order of `books` found by a beam search of `width`, by
    /// greedy search, or reading the easiest book now first, whichever is
    /// best (a beam can lose either order). Every one reads a series in
    /// volume order, and with [`Params::keep_series`], with no other book
    /// between its volumes. `None` if cancelled.
    pub fn optimise(
        &self,
        known0: &Known,
        params: &Params,
        books: &[usize],
        width: usize,
        cancel: &AtomicBool,
    ) -> Option<Path> {
        self.optimise_with_baseline(known0, params, books, width, cancel)
            .map(|(best, _)| best)
    }

    /// [`Library::optimise`], also returning the evaluated easiest-now baseline
    /// path so callers that compare against it don't evaluate it twice.
    pub fn optimise_with_baseline(
        &self,
        known0: &Known,
        params: &Params,
        books: &[usize],
        width: usize,
        cancel: &AtomicBool,
    ) -> Option<(Path, Path)> {
        let read_cache = self.build_read_cache(books, params);
        let rc = read_cache.as_deref();
        let first = self.first_books(params, books);
        let mut found: Vec<(Vec<usize>, Key)> = Vec::with_capacity(2);
        if width > 1 {
            found.push(self.beam(known0, params, books, &first, width, rc, cancel)?);
        }
        found.push(self.beam(known0, params, books, &first, 1, rc, cancel)?);
        let easiest_order = self.started_first(params, &self.easiest_now(known0, books));
        let naive = self.evaluate_with(known0, params, &easiest_order, rc);
        let naive_key = naive.key();
        // The first of the best: the beam's when it ties.
        let best_beam = found.into_iter().min_by_key(|(_, k)| *k);
        let best = match best_beam {
            Some((order, key)) if key <= naive_key => {
                if order == easiest_order {
                    naive.clone()
                } else {
                    self.evaluate_with(known0, params, &order, rc)
                }
            }
            _ => naive.clone(),
        };
        Some((best, naive))
    }

    /// `first`: books read first, in that order, before the search.
    ///
    /// The partial paths are kept by how many books they have read
    /// (`levels`), and only paths of as many books are compared. A book
    /// leads to the paths one longer; keeping series together, a whole
    /// series ([`Library::blocks`]) leads to the paths as many books longer.
    #[allow(clippy::too_many_arguments)]
    fn beam(
        &self,
        known0: &Known,
        params: &Params,
        books: &[usize],
        first: &[usize],
        width: usize,
        read_cache: Option<&[Option<BookReadCache>]>,
        cancel: &AtomicBool,
    ) -> Option<(Vec<usize>, Key)> {
        let width = width.max(1);
        let words = (self.books.len()).div_ceil(64);
        let mut start = State {
            order: Vec::with_capacity(books.len()),
            known: known0.clone(),
            future: if params.lookahead > 0.0 {
                self.future(books)
            } else {
                Vec::new()
            },
            done: vec![0; words],
            key: EMPTY,
        };
        for &b in first {
            let c = self.step_cost_only(b, &mut start.known, &mut start.future, params, read_cache);
            start.key = add_cost(start.key, c);
            start.done[b / 64] |= 1 << (b % 64);
            start.order.push(b);
        }
        let in_done = |d: &[u64], b: usize| d[b / 64] >> (b % 64) & 1 == 1;
        let with = |d: &[u64], bs: &[usize]| {
            let mut d = d.to_vec();
            for &b in bs {
                d[b / 64] |= 1 << (b % 64);
            }
            d
        };
        let earlier = self.earlier_volumes(books);
        let blocks = self.blocks(params, books);
        let mut in_block = vec![false; self.books.len()];
        for &b in blocks.iter().flatten() {
            in_block[b] = true;
        }
        let rank = |v: &mut Vec<State>| {
            v.sort_by_key(|s| order_key(params.objective, s.key));
            let mut seen: FxHashSet<Vec<u64>> = FxHashSet::default();
            v.retain(|s| seen.insert(s.done.clone()));
            v.truncate(width);
        };
        let mut levels: Vec<Vec<State>> = (0..=books.len()).map(|_| Vec::new()).collect();
        levels[first.len().min(books.len())].push(start);
        for at in first.len()..books.len() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let mut beam = std::mem::take(&mut levels[at]);
            rank(&mut beam);
            if beam.is_empty() {
                continue;
            }
            // Every (state, next book), ranked by the cheap cost. A book of a
            // series waits for its earlier volumes; one of them is always
            // free, since volumes are ordered. Keeping series together, a
            // series' books come only as the whole series, below.
            let mut cands: Vec<(Key, usize, usize)> = beam
                .par_iter()
                .enumerate()
                .flat_map(|(si, s)| {
                    let earlier = &earlier;
                    let in_block = &in_block;
                    books
                        .par_iter()
                        .with_min_len(8)
                        .copied()
                        .filter(move |&b| {
                            !in_block[b]
                                && !in_done(&s.done, b)
                                && earlier[b].iter().all(|&a| in_done(&s.done, a))
                        })
                        .map(move |b| (si, b))
                })
                .map(|(si, b)| {
                    let s = &beam[si];
                    let c = self.cheap_cost(b, &s.known, params.study, read_cache);
                    (order_key(params.objective, add_cost(s.key, c)), si, b)
                })
                .collect();
            cands.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
            // The most promising distinct sets of books, made real.
            let mut seen: FxHashSet<Vec<u64>> = FxHashSet::default();
            let picked: Vec<(usize, usize)> = cands
                .iter()
                .filter(|(_, si, b)| seen.insert(with(&beam[*si].done, &[*b])))
                .take(width * 2)
                .map(|(_, si, b)| (*si, *b))
                .collect();
            let next: Vec<State> = picked
                .par_iter()
                .map(|&(si, b)| self.advance(&beam[si], &[b], params, read_cache))
                .collect();
            levels[at + 1].extend(next);
            if blocks.is_empty() {
                continue;
            }
            // Each series not yet read, read whole from each state: its real
            // cost, for the paths of as many books more (the states it leads
            // to are made again, as only the best are kept).
            let mut whole: Vec<(Key, usize, usize)> = beam
                .par_iter()
                .enumerate()
                .flat_map(|(si, s)| {
                    let blocks = &blocks;
                    blocks
                        .par_iter()
                        .enumerate()
                        .filter(move |(_, block)| !block.iter().any(|&b| in_done(&s.done, b)))
                        .map(move |(bi, _)| (si, bi))
                })
                .map(|(si, bi)| {
                    let k = self.advance_key(&beam[si], &blocks[bi], params, read_cache);
                    (order_key(params.objective, k), si, bi)
                })
                .collect();
            whole.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
            // At most `width` distinct sets of books for each length.
            let mut seen: FxHashSet<Vec<u64>> = FxHashSet::default();
            let mut per_level: FxHashMap<usize, usize> = FxHashMap::default();
            let picked: Vec<(usize, usize)> = whole
                .iter()
                .filter(|(_, si, bi)| {
                    let n = per_level.entry(blocks[*bi].len()).or_default();
                    *n < width && seen.insert(with(&beam[*si].done, &blocks[*bi])) && {
                        *n += 1;
                        true
                    }
                })
                .map(|(_, si, bi)| (*si, *bi))
                .collect();
            let made: Vec<State> = picked
                .par_iter()
                .map(|&(si, bi)| self.advance(&beam[si], &blocks[bi], params, read_cache))
                .collect();
            for s in made {
                let to = s.order.len();
                levels[to].push(s);
                if levels[to].len() > width * 2 {
                    rank(&mut levels[to]);
                }
            }
        }
        let mut last = std::mem::take(&mut levels[books.len()]);
        rank(&mut last);
        last.into_iter()
            .next()
            .map(|s| (s.order, order_key(params.objective, s.key)))
    }

    /// `s` after reading `next`, in that order.
    fn advance(
        &self,
        s: &State,
        next: &[usize],
        params: &Params,
        read_cache: Option<&[Option<BookReadCache>]>,
    ) -> State {
        let mut s = s.clone();
        for &b in next {
            let c = self.step_cost_only(b, &mut s.known, &mut s.future, params, read_cache);
            s.key = add_cost(s.key, c);
            s.done[b / 64] |= 1 << (b % 64);
            s.order.push(b);
        }
        s
    }

    /// The key of `s` after reading `next` ([`Library::advance`] without
    /// keeping the steps).
    fn advance_key(
        &self,
        s: &State,
        next: &[usize],
        params: &Params,
        read_cache: Option<&[Option<BookReadCache>]>,
    ) -> Key {
        let mut known = s.known.clone();
        let mut future = s.future.clone();
        next.iter().fold(s.key, |k, &b| {
            let c = self.step_cost_only(b, &mut known, &mut future, params, read_cache);
            add_cost(k, c)
        })
    }
}

/// Known running words still needed to reach `target` percent.
fn need_tokens(target: f64, known: u64, total: u64) -> u64 {
    let want = (target / 100.0 * total as f64 - 1e-9).ceil().max(0.0) as u64;
    want.min(total).saturating_sub(known)
}

/// Word lookups a search may take: a few seconds on a desktop CPU.
pub const SEARCH_BUDGET: f64 = 2e9;

/// A beam width that keeps a search near `budget` word lookups.
pub fn width_for(lib: &Library, books: &[usize], budget: f64) -> usize {
    let n = books.len().max(1) as f64;
    // Learning while reading also goes through where words occur.
    let avg = books
        .iter()
        .map(|&b| {
            let bk = &lib.books[b];
            bk.words.len() as f64
                + bk.pos
                    .as_ref()
                    .map_or(0, |p| p.words.iter().map(Vec::len).sum::<usize>())
                    as f64
        })
        .sum::<f64>()
        / n;
    // A search of width W scans about W * n^2 / 2 books.
    let w = budget / (avg.max(1.0) * n * n / 2.0);
    (w as usize).clamp(1, 32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn profile(id: &str, words: &[(&str, u32)], names: &[&str]) -> (String, Profile) {
        let mut w: Vec<(String, u32)> = words.iter().map(|(a, b)| (a.to_string(), *b)).collect();
        w.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        (
            id.to_uppercase(),
            Profile {
                format: crate::profile::FORMAT,
                id: id.into(),
                variant: "Simplified".into(),
                segmenter: "LTP".into(),
                words: w,
                names: names.iter().map(|s| s.to_string()).collect(),
                chars: None,
            },
        )
    }

    fn params(study: Study, lookahead: f32) -> Params {
        Params {
            study,
            objective: Objective::Peak,
            incidental: None,
            lookahead,
            keep_series: false,
        }
    }

    #[test]
    fn target_takes_the_fewest_frequent_words() {
        let lib = Library::build(
            false,
            false,
            vec![profile("a", &[("甲", 50), ("乙", 30), ("丙", 20)], &[])],
        );
        let k0 = lib.known(None);
        let p = lib.evaluate(&k0, &params(Study::Target(80.0), 0.0), &[0]);
        let s = &p.steps[0];
        assert_eq!(
            lib.study_list(0, &s.study),
            [("甲".into(), 50), ("乙".into(), 30)]
        );
        assert_eq!((s.now, s.reading), (0.0, 80.0));
        assert_eq!(s.study_now, Some(2));
        // Nothing to study once the target is met.
        let p = lib.evaluate(&k0, &params(Study::Target(0.0), 0.0), &[0]);
        assert!(p.steps[0].study.is_empty());
    }

    #[test]
    fn style_offset_raises_the_target() {
        let words = [("甲", 50), ("乙", 30), ("丙", 20)];
        let mut lib = Library::build(
            false,
            false,
            vec![profile("a", &words, &[]), profile("b", &words, &[])],
        );
        assert!(lib.set_offsets(|id| if id == "b" { 5.0 } else { 0.0 }));
        assert!(!lib.set_offsets(|id| if id == "b" { 5.0 } else { 0.0 }));
        let k0 = lib.known(None);
        // 80% takes 甲 and 乙; 85% for b also takes 丙.
        let p = lib.evaluate(&k0, &params(Study::Target(80.0), 0.0), &[0]);
        assert_eq!(p.steps[0].study.len(), 2);
        let p = lib.evaluate(&k0, &params(Study::Target(80.0), 0.0), &[1]);
        assert_eq!(p.steps[0].study.len(), 3);
        assert_eq!(p.steps[0].offset, 5.0);
        // Equal coverage: the easier style comes first.
        assert_eq!(lib.easiest_now(&k0, &[1, 0]), [0, 1]);
        // Under a fixed number the offset is taken off the reading coverage.
        let pr = params(Study::Fixed(1), 0.0);
        let a = lib.evaluate(&k0, &pr, &[0]);
        let b = lib.evaluate(&k0, &pr, &[1]);
        assert_eq!(a.steps[0].reading, b.steps[0].reading);
        assert!(b.key() > a.key());
    }

    #[test]
    fn names_are_never_studied_and_left_out() {
        let lib = Library::build(
            false,
            false,
            vec![profile(
                "a",
                &[("张三", 60), ("说", 30), ("走", 10)],
                &["张三"],
            )],
        );
        let k0 = lib.known(None);
        let p = lib.evaluate(&k0, &params(Study::Target(75.0), 0.0), &[0]);
        let s = &p.steps[0];
        // 张三 is out of the running words: 说 alone is 75% of 40.
        assert_eq!(lib.study_list(0, &s.study), [("说".into(), 30)]);
        assert_eq!(s.reading, 75.0);
        // A known name counts as known.
        let mut k = Knowledge::empty(false);
        k.add_listed(["张三"], "x".into());
        let k0 = lib.known(Some(&k));
        let p = lib.evaluate(&k0, &params(Study::Fixed(0), 0.0), &[0]);
        assert_eq!(p.steps[0].now, 60.0);
    }

    #[test]
    fn hapax_left_out_when_asked() {
        let books = || vec![profile("a", &[("说", 8), ("走", 1), ("跑", 1)], &[])];
        let all = Library::build(false, false, books());
        assert_eq!(
            (all.books[0].unique_words(), all.books[0].running_words()),
            (3, 10)
        );
        let lib = Library::build(false, true, books());
        assert_eq!(
            (lib.books[0].unique_words(), lib.books[0].running_words()),
            (1, 8)
        );
        let k0 = lib.known(None);
        let p = lib.evaluate(&k0, &params(Study::Target(100.0), 0.0), &[0]);
        assert_eq!(lib.study_list(0, &p.steps[0].study), [("说".into(), 8)]);
    }

    #[test]
    fn folding_merges_traditional_and_simplified() {
        let lib = Library::build(
            true,
            false,
            vec![
                profile("s", &[("说话", 10)], &[]),
                profile("t", &[("說話", 10)], &[]),
            ],
        );
        let k0 = lib.known(None);
        let p = lib.evaluate(&k0, &params(Study::Target(100.0), 0.0), &[0, 1]);
        assert_eq!(p.steps[1].study.len(), 0);
        assert_eq!(p.steps[1].gained, 1);
        assert_eq!(p.steps[1].reached, 100.0);
        // Shown as the Traditional book spells it.
        assert_eq!(lib.study_list(1, &p.steps[0].study)[0].0, "說話");
    }

    const READ: Study = Study::Read {
        percent: 90,
        min_count: 10,
    };

    fn read(percent: u8, min_count: u32) -> Params {
        params(Study::Read { percent, min_count }, 0.0)
    }

    /// Words `prefix`0, `prefix`1, … each `n` times.
    fn many(prefix: &str, words: usize, n: u32) -> Vec<(String, u32)> {
        (0..words).map(|i| (format!("{prefix}{i}"), n)).collect()
    }

    fn owned(id: &str, words: Vec<(String, u32)>) -> (String, Profile) {
        let v: Vec<(&str, u32)> = words.iter().map(|(w, n)| (w.as_str(), *n)).collect();
        profile(id, &v, &[])
    }

    #[test]
    fn learning_while_reading_rewards_repetition() {
        // The same running words and none known: the long-running words of
        // one are met 20 times each, the other's twice.
        let lib = Library::build(
            false,
            false,
            vec![
                owned("rep", many("甲", 10, 20)),
                owned("var", many("乙", 100, 2)),
            ],
        );
        let k0 = lib.known(None);
        let p = lib.evaluate(&k0, &read(100, 10), &[0, 1]);
        let (a, b) = (&p.steps[0], &p.steps[1]);
        assert_eq!((a.now, b.now), (0.0, 0.0));
        // Each of the ten met unknown 10 times, then known.
        assert_eq!((a.learned.len(), a.lookups), (10, 100));
        assert!((a.reading - 50.0).abs() < 1e-9 && (a.start - 50.0).abs() < 1e-9);
        assert_eq!((b.learned.len(), b.lookups), (0, 200));
        assert_eq!(b.reading, 0.0);
        assert!(b.study.is_empty() && b.study_now.is_none());
        assert_eq!(p.total_lookups(), 300);
        assert_eq!(p.lowest_start(), 0.0);
        assert_eq!(p.key(), [0, 300]);
    }

    #[test]
    fn learning_while_reading_carries_over() {
        let a = profile("a", &[("甲", 20), ("乙", 20), ("丙", 20)], &[]);
        let b = profile("b", &[("甲", 5), ("乙", 5), ("丁", 5)], &[]);
        let c = profile("c", &[("戊", 30), ("己", 30)], &[]);
        let lib = Library::build(false, false, vec![b, a, c]);
        let k0 = lib.known(None);
        let pr = read(100, 10);
        let best = lib
            .optimise(&k0, &pr, &[0, 1, 2], 8, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(best.key(), brute_force(&lib, &k0, &pr, &[0, 1, 2]).key());
        let pos = |x: usize| best.order().iter().position(|&b| b == x).unwrap();
        // A teaches 甲 乙 before B: B starts at two thirds known, not none.
        assert!(pos(1) < pos(0));
        let sb = &best.steps[pos(0)];
        assert!((sb.reached - 200.0 / 3.0).abs() < 1e-9);
        assert_eq!(sb.gained, 2);
        // 丁 met 5 times teaches nothing yet; 甲 乙 were taught by A.
        assert!(sb.learned.is_empty());
    }

    /// Marking the path's books read one after another, as ✔ Mark read
    /// does (with words across books), knows what the path said it would.
    #[test]
    fn learning_while_reading_matches_mark_read() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        let pool: Vec<String> = (0..50)
            .map(|i| char::from_u32(0x4e00 + i).unwrap().to_string())
            .collect();
        for round in 0..20 {
            let profiles: Vec<(String, Profile)> = (0..5)
                .map(|b| {
                    let mut m: HashMap<&str, u32> = HashMap::new();
                    for _ in 0..30 {
                        let w = &pool[(rnd(30) + b * 5) as usize % pool.len()];
                        *m.entry(w).or_default() += 1 + (20 / (1 + rnd(15))) as u32;
                    }
                    let v: Vec<(&str, u32)> = m.into_iter().collect();
                    // A name in some: taken by Mark read like any word.
                    let names: &[&str] = if b % 2 == 0 { &["一丁"] } else { &[] };
                    let mut v = v;
                    if b % 2 == 0 {
                        v.push(("一丁", 12));
                    }
                    profile(["p", "q", "r", "s", "t"][b as usize], &v, names)
                })
                .collect();
            let lib = Library::build(false, false, profiles.clone());
            // Some words known at the start.
            let mut ks = crate::known::Settings::default();
            for w in pool.iter().step_by(7) {
                ks.set_marked(w, true);
            }
            let k0 = lib.known(ks.apply(None, false).as_ref());
            let (percent, min_count) = (80 + (round % 3) as u8 * 10, 6 + round % 5);
            let pr = read(percent, min_count);
            let order: Vec<usize> = lib
                .optimise(&k0, &pr, &[0, 1, 2, 3, 4], 4, &AtomicBool::new(false))
                .unwrap()
                .order();
            let path = lib.evaluate(&k0, &pr, &order);
            let mut predicted: FxHashSet<u32> =
                (0..lib.vocab_len() as u32).filter(|&w| k0.has(w)).collect();
            let mut counts: HashMap<String, Vec<(String, u32)>> = HashMap::new();
            for (i, s) in path.steps.iter().enumerate() {
                predicted.extend(s.learned.iter().copied());
                let p = &profiles[s.book].1;
                let words: Vec<String> = crate::known::read_selection(&p.words, percent, min_count)
                    .into_iter()
                    .map(|(w, _)| w.clone())
                    .collect();
                ks.set_read(crate::known::ReadBook {
                    id: p.id.clone(),
                    words,
                    percent,
                    min_count,
                    ..Default::default()
                });
                counts.insert(p.id.clone(), p.words.clone());
                let input: Vec<crate::known::ReadCounts> = ks
                    .read
                    .iter()
                    .map(|m| (&counts[&m.id][..], &m.words[..]))
                    .collect();
                ks.across = crate::known::across_books(&input, min_count, false);
                let marked = lib.known(ks.apply(None, false).as_ref());
                let got: FxHashSet<u32> = (0..lib.vocab_len() as u32)
                    .filter(|&w| marked.has(w))
                    .collect();
                assert_eq!(got, predicted, "round {round}, after step {i}");
            }
        }
    }

    /// One book of two sections, 20,000 and 80,000 running words: 的 in both,
    /// a hundred rare words in the first (`front`) or the second.
    fn two_part(id: &str, front: bool) -> (String, Profile, Option<crate::profile::Positions>) {
        use crate::profile::{PosSection, Positions};
        let mut words = vec![("的".to_owned(), 90_000u32)];
        words.extend(many("稀", 100, 100));
        let (_, p) = owned(id, words);
        let sections = vec![
            PosSection {
                href: Some("a.xhtml".into()),
                start: 0,
                words: 20_000,
            },
            PosSection {
                href: Some("b.xhtml".into()),
                start: 20_000,
                words: 80_000,
            },
        ];
        let pw: Vec<Vec<(u32, u32)>> = p
            .words
            .iter()
            .map(|(w, n)| match (w.as_str(), front) {
                ("的", true) => vec![(0, 10_000)],
                ("的", false) => vec![(0, 20_000)],
                (_, true) => vec![(0, *n)],
                (_, false) => vec![(1, *n)],
            })
            .collect();
        (
            id.to_uppercase(),
            p,
            Some(Positions {
                sections,
                words: pw,
            }),
        )
    }

    #[test]
    fn start_counts_where_words_occur() {
        let mut no_pos = two_part("even", true);
        no_pos.2 = None;
        let lib = Library::build_with(
            false,
            false,
            vec![two_part("front", true), two_part("back", false), no_pos],
        );
        let k0 = lib.known(None);
        // 的 learned after 1,000 meetings; the rare words never.
        let pr = read(100, 1_000);
        let at = |b: usize| lib.evaluate(&k0, &pr, &[b]).steps[0].clone();
        let (front, back, even) = (at(0), at(1), at(2));
        // Front: all 10,000 rare and 1,000 的 unknown in the first 20,000.
        assert!((front.start - 45.0).abs() < 1e-6, "{}", front.start);
        assert!((back.start - 95.0).abs() < 1e-6, "{}", back.start);
        // Even spread: a fifth of the rare words.
        assert!((even.start - 85.0).abs() < 1e-6, "{}", even.start);
        // Over the whole book the same: 11,000 of 100,000.
        for s in [&front, &back, &even] {
            assert!((s.reading - 89.0).abs() < 1e-6);
            assert_eq!(s.lookups, 11_000);
        }
        assert!(lib.books[0].has_positions() && !lib.books[2].has_positions());
        // The easier start first.
        let best = lib
            .optimise(&k0, &pr, &[0, 1], 4, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(best.order(), [1, 0]);
    }

    #[test]
    fn a_book_started_comes_first() {
        let mut lib = Library::build_with(
            false,
            false,
            vec![two_part("front", true), two_part("back", false)],
        );
        let pr = read(100, 1_000);
        let k0 = lib.known(None);
        // Under 1% in: not started.
        assert!(!lib.set_reading(|b| (b.id == "front").then_some((999, 5))));
        assert!(lib.started(&pr, &[0, 1]).is_empty());
        // Past the first section of the harder book: it comes first, and
        // what is left of it is easy.
        assert!(lib.set_reading(|b| (b.id == "front").then_some((20_000, 5))));
        assert_eq!(lib.started(&pr, &[0, 1]), [0]);
        assert!(
            lib.started(&params(Study::Target(90.0), 0.0), &[0, 1])
                .is_empty()
        );
        let best = lib
            .optimise(&k0, &pr, &[0, 1], 4, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(best.order(), [0, 1]);
        let s = &best.steps[0];
        assert_eq!(s.from, Some(0.2));
        // 的 met 10,000 times already: learned; nothing unknown left to meet.
        assert_eq!((s.learned_so_far, s.learned.len()), (1, 1));
        assert_eq!((s.start, s.reading, s.lookups), (100.0, 100.0, 0));
        // The other book benefits: 的 known.
        assert!(best.steps[1].start > 99.0);
        // Two started: the one read last first.
        lib.set_reading(|b| Some((30_000, if b.id == "back" { 9 } else { 5 })));
        assert_eq!(lib.started(&pr, &[0, 1]), [1, 0]);
        assert_eq!(lib.started_first(&pr, &[0, 1]), [1, 0]);
    }

    #[test]
    fn settings_default_to_learning_while_reading() {
        let s: Settings = serde_json::from_str(r#"{"fixed": true, "learn": 50}"#).unwrap();
        assert_eq!(
            (s.mode, s.objective, s.fixed, s.learn),
            (Mode::Read, Objective::Peak, true, 50)
        );
        assert_eq!(
            s.params().study,
            Study::Read {
                percent: 90,
                min_count: 10
            }
        );
        let mut t = s.clone();
        t.mode = Mode::Study;
        t.objective = Objective::Total;
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains(r#""mode":"study""#) && json.contains(r#""objective":"total""#));
        let u: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(u, t);
        assert_eq!(u.params().study, Study::Fixed(50));
        assert_eq!(u.params().objective, Objective::Total);
    }

    #[test]
    fn keep_series_is_saved_and_off_in_older_files() {
        let s: Settings = serde_json::from_str(r#"{"mode": "study", "target": 98.0}"#).unwrap();
        assert!(!s.keep_series && !s.params().keep_series);
        let mut t = s.clone();
        t.keep_series = true;
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains(r#""keep_series":true"#));
        let u: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(u, t);
        assert!(u.params().keep_series);
        t.mode = Mode::Read;
        assert!(t.params().keep_series);
    }

    #[test]
    fn small_library_matches_every_order() {
        // B shares its vocabulary with A and C: whichever comes first makes the others easier.
        let a = profile("a", &[("一", 40), ("二", 30), ("三", 30)], &[]);
        let b = profile("b", &[("一", 25), ("二", 25), ("四", 25), ("五", 25)], &[]);
        let c = profile("c", &[("四", 40), ("五", 30), ("六", 30)], &[]);
        let lib = Library::build(false, false, vec![a, b, c]);
        let k0 = lib.known(None);
        let pr = params(Study::Target(70.0), 0.0);
        let best = lib
            .optimise(&k0, &pr, &[0, 1, 2], 8, &AtomicBool::new(false))
            .unwrap();
        let brute = brute_force(&lib, &k0, &pr, &[0, 1, 2]);
        assert_eq!(best.key(), brute.key());
        // A (一 二), then B needs 四, then C needs 五.
        assert_eq!(best.key(), [2, 4]);
        assert_eq!(best.steps[1].gained, 2);
    }

    #[test]
    fn lookahead_prefers_words_needed_later() {
        let a = profile("a", &[("甲", 11), ("乙", 10), ("丙", 1)], &[]);
        let b = profile("b", &[("乙", 50), ("丁", 50)], &[]);
        let lib = Library::build(false, false, vec![a, b]);
        let k0 = lib.known(None);
        let p = lib.evaluate(&k0, &params(Study::Fixed(1), 0.0), &[0, 1]);
        assert_eq!(lib.word(p.steps[0].study[0]), "甲");
        let p = lib.evaluate(&k0, &params(Study::Fixed(1), 1.0), &[0, 1]);
        assert_eq!(lib.word(p.steps[0].study[0]), "乙");
        assert_eq!(p.steps[1].gained, 1);
    }

    #[test]
    fn incidental_learning() {
        let a = profile("a", &[("甲", 20), ("乙", 3)], &[]);
        let b = profile("b", &[("甲", 1), ("乙", 1)], &[]);
        let lib = Library::build(false, false, vec![a, b]);
        let k0 = lib.known(None);
        let mut pr = params(Study::Fixed(0), 0.0);
        pr.incidental = Some(10);
        let p = lib.evaluate(&k0, &pr, &[0, 1]);
        assert_eq!(p.steps[0].by_reading, 1);
        assert_eq!(p.steps[1].reached, 50.0);
    }

    /// The libraries hold the same numbered words, books and series.
    fn same_library(a: &Library, b: &Library) {
        assert_eq!(a.fold, b.fold);
        assert_eq!(a.vocab, b.vocab);
        assert_eq!(a.books.len(), b.books.len());
        for (x, y) in a.books.iter().zip(&b.books) {
            assert_eq!(
                (&x.id, &x.title, &x.segmenter, &x.variant, &x.series),
                (&y.id, &y.title, &y.segmenter, &y.variant, &y.series)
            );
            assert_eq!(*x.data, *y.data, "{}", x.id);
        }
    }

    /// A book of `n` words drawn from a small pool (Traditional and
    /// Simplified spellings, a name, tied counts), with positions when `pos`.
    fn random_book(
        id: &str,
        seed: &mut u64,
        n: usize,
        pos: bool,
    ) -> (String, Profile, Option<crate::profile::Positions>) {
        use crate::profile::{PosSection, Positions};
        const POOL: &[&str] = &[
            "说",
            "說",
            "书",
            "書",
            "我们",
            "他们",
            "张三",
            "张三说",
            "天气",
            "学校",
            "學校",
            "朋友",
            "时间",
            "時間",
            "问题",
            "問題",
            "东西",
            "東西",
            "孩子",
            "地方",
            "工作",
            "电话",
            "電話",
        ];
        let mut next = || {
            *seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (*seed >> 33) as usize
        };
        let mut words: Vec<(&str, u32)> = Vec::new();
        for _ in 0..n {
            let w = POOL[next() % POOL.len()];
            if !words.iter().any(|x| x.0 == w) {
                // Few distinct counts: many ties.
                words.push((w, [1, 2, 3, 5, 5, 8][next() % 6]));
            }
        }
        let (_, p) = profile(id, &words, &["张三"]);
        let positions = pos.then(|| Positions {
            sections: vec![
                PosSection {
                    href: Some("a.xhtml".into()),
                    start: 0,
                    words: 30,
                },
                PosSection {
                    href: Some("b.xhtml".into()),
                    start: 30,
                    words: 70,
                },
            ],
            words: p
                .words
                .iter()
                .map(|(_, n)| match n / 2 {
                    0 => vec![(1, *n)],
                    h => vec![(0, h), (1, n - h)],
                })
                .collect(),
        });
        (
            format!("{} 第{}卷", id.to_uppercase(), id.len()),
            p,
            positions,
        )
    }

    #[test]
    fn rebuilding_keeps_books_and_numbers_words_as_a_full_build() {
        let mut seed = 7u64;
        for fold in [true, false] {
            for skip_hapax in [false, true] {
                let mut books: Vec<(String, Profile, Option<crate::profile::Positions>)> = (0..6)
                    .map(|i| random_book(&format!("b{i}"), &mut seed, 8 + i, i % 2 == 0))
                    .collect();
                let mut old = Library::build_with(fold, skip_hapax, books.clone());
                for round in 0..40 {
                    // Change one thing: a book's words, a book added, removed,
                    // moved, or retitled.
                    let changed: Option<String> = match round % 5 {
                        0 => {
                            let i = round % books.len();
                            let id = books[i].1.id.clone();
                            books[i] = random_book(&id, &mut seed, 10, round % 2 == 0);
                            Some(id)
                        }
                        1 => {
                            let id = format!("n{round}");
                            books.insert(
                                round % (books.len() + 1),
                                random_book(&id, &mut seed, 9, true),
                            );
                            Some(id)
                        }
                        2 if books.len() > 2 => {
                            books.remove(round % books.len());
                            None
                        }
                        3 => {
                            let b = books.remove(0);
                            books.push(b);
                            None
                        }
                        _ => {
                            let i = round % books.len();
                            books[i].0 = format!("Retitled {round} 第2卷");
                            None
                        }
                    };
                    let index: HashMap<&str, usize> = old
                        .books
                        .iter()
                        .enumerate()
                        .map(|(i, b)| (b.id.as_str(), i))
                        .collect();
                    let src: Vec<(String, Source)> = books
                        .iter()
                        .map(|(t, p, pos)| {
                            let kept = (changed.as_deref() != Some(p.id.as_str()))
                                .then(|| index.get(p.id.as_str()))
                                .flatten();
                            let s = match kept {
                                Some(&i) => Source::Kept(i),
                                None => Source::Read(p.clone(), pos.clone()),
                            };
                            (t.clone(), s)
                        })
                        .collect();
                    let new = Library::rebuild(fold, skip_hapax, src, Some(&old));
                    same_library(&new, &Library::build_with(fold, skip_hapax, books.clone()));
                    old = new;
                }
            }
        }
    }

    #[test]
    fn a_kept_book_shares_its_words_when_numbered_alike() {
        let mut seed = 3u64;
        let books: Vec<_> = (0..4)
            .map(|i| random_book(&format!("b{i}"), &mut seed, 12, true))
            .collect();
        let old = Library::build_with(true, false, books.clone());
        // The last book read again: the others keep their numbers and data.
        let src: Vec<(String, Source)> = books
            .iter()
            .enumerate()
            .map(|(i, (t, p, pos))| {
                let s = if i == 3 {
                    Source::Read(p.clone(), pos.clone())
                } else {
                    Source::Kept(i)
                };
                (t.clone(), s)
            })
            .collect();
        let new = Library::rebuild(true, false, src, Some(&old));
        for i in 0..3 {
            assert!(Arc::ptr_eq(&old.books[i].data, &new.books[i].data));
        }
        same_library(&new, &old);
        // A copy to set offsets shares every book's words.
        let mut copy = Arc::new(new);
        let keep = copy.clone();
        Arc::make_mut(&mut copy).set_offsets(|_| 1.0);
        assert!(Arc::ptr_eq(&copy.vocab, &keep.vocab));
        assert!(Arc::ptr_eq(&copy.books[0].data, &keep.books[0].data));
    }

    fn titled(title: &str, words: &[(&str, u32)]) -> (String, Profile) {
        let (_, p) = profile(
            &format!("{:x}", title.len() * 7919 + title.chars().count()),
            words,
            &[],
        );
        (
            title.into(),
            Profile {
                id: title.into(),
                ..p
            },
        )
    }

    fn in_series_order(lib: &Library, order: &[usize]) -> bool {
        order
            .iter()
            .enumerate()
            .all(|(i, &b)| order[i + 1..].iter().all(|&a| !lib.precedes(a, b)))
    }

    #[test]
    fn series_are_read_in_volume_order() {
        // Volume 2 is the easiest and teaches most of volume 1, so without
        // the series it would be read first.
        let books = vec![
            titled("SCI谜案集（第二部）", &[("一", 50), ("二", 40), ("三", 10)]),
            titled(
                "SCI谜案集（第一部）",
                &[("一", 30), ("二", 30), ("四", 20), ("五", 20)],
            ),
            titled("其他", &[("六", 50), ("七", 50)]),
            titled("SCI谜案集 （第四部）", &[("一", 60), ("八", 40)]),
            titled("SCI謎案集(第3部)", &[("一", 40), ("九", 30), ("十", 30)]),
        ];
        let lib = Library::build(false, false, books);
        let series: Vec<Option<Vec<u32>>> = lib
            .books
            .iter()
            .map(|b| b.series.as_ref().map(|s| s.1.clone()))
            .collect();
        assert_eq!(
            series,
            [
                Some(vec![20]),
                Some(vec![10]),
                None,
                Some(vec![40]),
                Some(vec![30])
            ]
        );
        let k0 = lib.known(None);
        let all: Vec<usize> = (0..5).collect();
        let none = AtomicBool::new(false);
        for study in [Study::Target(90.0), Study::Fixed(1)] {
            for objective in [Objective::Peak, Objective::Total] {
                let mut pr = params(study, 0.0);
                pr.objective = objective;
                for width in [1, 16] {
                    let p = lib.optimise(&k0, &pr, &all, width, &none).unwrap();
                    let o = p.order();
                    assert_eq!(
                        o.iter().copied().filter(|&b| b != 2).collect::<Vec<_>>(),
                        [1, 0, 4, 3],
                        "{study:?} {objective:?} width {width}: {o:?}"
                    );
                }
            }
        }
        // Without the series, volume 2 is read before volume 1.
        let mut plain = lib.clone();
        for b in &mut plain.books {
            b.series = None;
        }
        let p = plain
            .optimise(&k0, &params(Study::Target(90.0), 0.0), &all, 16, &none)
            .unwrap();
        let o = p.order();
        let at = |b| o.iter().position(|&x| x == b).unwrap();
        assert!(at(0) < at(1), "{o:?}");
        assert!(!in_series_order(&lib, &plain.easiest_now(&k0, &all)));
        let easy = lib.easiest_now(&k0, &all);
        assert!(in_series_order(&lib, &easy), "{easy:?}");
        // A hand-set order keeps its other books in place.
        assert_eq!(lib.in_series_order(&[3, 2, 0, 1, 4]), [1, 2, 0, 4, 3]);
    }

    /// Random libraries with series: every path keeps them in order, and
    /// the beam is close to the best valid order.
    #[test]
    fn beam_with_series_against_every_valid_order() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        let pool: Vec<String> = (0..50)
            .map(|i| char::from_u32(0x4e00 + i).unwrap().to_string())
            .collect();
        let titles = ["甲 1", "乙（上）", "甲 第三部", "丙", "乙（下）", "甲 2"];
        let (mut optimal, mut runs) = (0, 0);
        for _ in 0..20 {
            let books: Vec<(String, Profile)> = titles
                .iter()
                .enumerate()
                .map(|(b, t)| {
                    let mut m: HashMap<&str, u32> = HashMap::new();
                    for _ in 0..20 {
                        let w = &pool[(rnd(30) + b as u64 * 4) as usize % pool.len()];
                        *m.entry(w).or_default() += 1 + (50 / (1 + rnd(25))) as u32;
                    }
                    let v: Vec<(&str, u32)> = m.into_iter().collect();
                    titled(t, &v)
                })
                .collect();
            let lib = Library::build(false, false, books);
            assert!(lib.precedes(0, 5) && lib.precedes(5, 2) && lib.precedes(1, 4));
            assert!(lib.books[3].series.is_none());
            let k0 = lib.known(None);
            let idx: Vec<usize> = (0..6).collect();
            for study in [Study::Target(90.0), Study::Fixed(3)] {
                let pr = params(study, 0.5);
                let none = AtomicBool::new(false);
                let greedy = lib.optimise(&k0, &pr, &idx, 1, &none).unwrap();
                let beam = lib.optimise(&k0, &pr, &idx, 16, &none).unwrap();
                for p in [&greedy, &beam] {
                    assert!(in_series_order(&lib, &p.order()), "{:?}", p.order());
                }
                assert!(beam.key() <= greedy.key());
                let best = valid_orders(&lib, &idx)
                    .into_iter()
                    .map(|o| lib.evaluate(&k0, &pr, &o))
                    .min_by_key(|p| p.key())
                    .unwrap();
                assert!(best.key() <= beam.key());
                runs += 1;
                optimal += usize::from(best.key() == beam.key());
            }
        }
        assert!(
            optimal * 10 >= runs * 7,
            "beam optimal in only {optimal} of {runs}"
        );
    }

    /// Each series' books of `order` come one after another, in volume order.
    fn series_kept_together(lib: &Library, order: &[usize]) -> bool {
        let mut seen: Vec<u32> = Vec::new();
        let mut last: Option<u32> = None;
        for &b in order {
            let s = lib.books[b].series.as_ref().map(|x| x.0);
            if let Some(s) = s
                && s != last.unwrap_or(u32::MAX)
            {
                if seen.contains(&s) {
                    return false;
                }
                seen.push(s);
            }
            last = s;
        }
        in_series_order(lib, order)
    }

    #[test]
    fn keep_series_keeps_a_series_together() {
        // 乙 shares 三 四 with 甲 2: reading it between the volumes makes
        // the largest step 2 words, keeping 甲 together 3.
        let books = vec![
            titled(
                "甲 2",
                &[("一", 10), ("二", 10), ("三", 10), ("四", 10), ("五", 10)],
            ),
            titled("乙", &[("一", 10), ("二", 10), ("三", 10), ("四", 10)]),
            titled("甲 1", &[("一", 10), ("二", 10)]),
        ];
        let lib = Library::build(false, false, books);
        assert!(lib.precedes(2, 0) && lib.same_series(0, 2) && !lib.same_series(0, 1));
        let k0 = lib.known(None);
        let all = [0, 1, 2];
        let none = AtomicBool::new(false);
        let mut pr = params(Study::Target(100.0), 0.0);
        for width in [1, 8] {
            let split = lib.optimise(&k0, &pr, &all, width, &none).unwrap();
            assert_eq!(split.order(), [2, 1, 0], "width {width}");
            assert_eq!(split.key(), [2, 5]);
        }
        pr.keep_series = true;
        for width in [1, 8] {
            let kept = lib.optimise(&k0, &pr, &all, width, &none).unwrap();
            assert_eq!(kept.order(), [2, 0, 1], "width {width}");
            assert_eq!(kept.key(), [3, 5]);
            // 甲 1's words are known for 甲 2.
            assert_eq!(kept.steps[1].gained, 2);
        }
        // Every mode and objective keeps it together.
        for study in [Study::Target(90.0), Study::Fixed(1), READ] {
            for objective in [Objective::Peak, Objective::Total] {
                let mut pr = params(study, 0.0);
                pr.objective = objective;
                pr.keep_series = true;
                for width in [1, 8] {
                    let o = lib.optimise(&k0, &pr, &all, width, &none).unwrap().order();
                    assert!(
                        series_kept_together(&lib, &o),
                        "{study:?} {objective:?} width {width}: {o:?}"
                    );
                }
            }
        }
        // Easiest first: the series where its easiest volume is.
        assert_eq!(lib.series_together(&[1, 0, 2]), [1, 2, 0]);
        assert_eq!(lib.started_first(&pr, &[0, 1, 2]), [2, 0, 1]);
    }

    #[test]
    fn a_started_volume_brings_the_rest_of_its_series() {
        let books = vec![
            titled("甲 1", &[("一", 10), ("二", 10)]),
            titled("乙", &[("一", 10), ("三", 10)]),
            titled("甲 3", &[("一", 10), ("四", 10)]),
            titled("甲 2", &[("一", 10), ("五", 10)]),
        ];
        let mut lib = Library::build(false, false, books);
        // Halfway through volume 2.
        assert!(lib.set_reading(|b| (b.title == "甲 2").then_some((10, 5))));
        let k0 = lib.known(None);
        let none = AtomicBool::new(false);
        let mut pr = read(100, 10);
        let all = [0, 1, 2, 3];
        assert_eq!(lib.first_books(&pr, &all), [3]);
        pr.keep_series = true;
        // Volume 3 next, then volume 1, which was not marked read.
        assert_eq!(lib.first_books(&pr, &all), [3, 2, 0]);
        for width in [1, 8] {
            let p = lib.optimise(&k0, &pr, &all, width, &none).unwrap();
            assert_eq!(p.order(), [3, 2, 0, 1]);
        }
        assert_eq!(lib.started_first(&pr, &[1, 0, 2, 3]), [3, 2, 0, 1]);
        // Studying before each book, nothing is started.
        let mut st = params(Study::Target(90.0), 0.0);
        st.keep_series = true;
        assert!(lib.first_books(&st, &all).is_empty());
    }

    /// Random libraries with series kept together: every path keeps them
    /// together and in order, and the beam is close to the best such order.
    #[test]
    fn keep_series_against_every_kept_order() {
        let mut seed = 0x1234_5678_9abc_def1u64;
        let mut rnd = |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        let pool: Vec<String> = (0..50)
            .map(|i| char::from_u32(0x4e00 + i).unwrap().to_string())
            .collect();
        let titles = [
            "甲 1",
            "乙（上）",
            "甲 第三部",
            "丙",
            "乙（下）",
            "甲 2",
            "丁",
        ];
        let (mut optimal, mut runs) = (0, 0);
        for round in 0..12 {
            let books: Vec<(String, Profile)> = titles
                .iter()
                .enumerate()
                .map(|(b, t)| {
                    let mut m: HashMap<&str, u32> = HashMap::new();
                    for _ in 0..20 {
                        let w = &pool[(rnd(30) + b as u64 * 4) as usize % pool.len()];
                        *m.entry(w).or_default() += 1 + (50 / (1 + rnd(25))) as u32;
                    }
                    let v: Vec<(&str, u32)> = m.into_iter().collect();
                    titled(t, &v)
                })
                .collect();
            let lib = Library::build(false, false, books);
            let k0 = lib.known(None);
            let idx: Vec<usize> = (0..titles.len()).collect();
            // Units: the two series, whole, and the other books.
            let units: Vec<Vec<usize>> = vec![vec![0, 5, 2], vec![1, 4], vec![3], vec![6]];
            let mut kept = Vec::new();
            let mut perm: Vec<usize> = (0..units.len()).collect();
            permute(&mut perm, 0, &mut |p| {
                kept.push(p.iter().flat_map(|&u| units[u].clone()).collect::<Vec<_>>())
            });
            for (study, objective) in [
                (Study::Target(90.0), Objective::Peak),
                (Study::Fixed(3), Objective::Total),
                (READ, Objective::Peak),
                (READ, Objective::Total),
            ] {
                let mut pr = params(study, if round % 2 == 0 { 0.0 } else { 0.5 });
                pr.objective = objective;
                pr.keep_series = true;
                let none = AtomicBool::new(false);
                let greedy = lib.optimise(&k0, &pr, &idx, 1, &none).unwrap();
                let beam = lib.optimise(&k0, &pr, &idx, 16, &none).unwrap();
                for p in [&greedy, &beam] {
                    assert!(series_kept_together(&lib, &p.order()), "{:?}", p.order());
                    assert_eq!(p.steps.len(), idx.len());
                }
                assert!(beam.key() <= greedy.key());
                let best = kept
                    .iter()
                    .map(|o| lib.evaluate(&k0, &pr, o))
                    .min_by_key(|p| p.key())
                    .unwrap();
                assert!(best.key() <= beam.key());
                runs += 1;
                optimal += usize::from(best.key() == beam.key());
            }
        }
        assert!(
            optimal * 10 >= runs * 8,
            "beam optimal in only {optimal} of {runs}"
        );
    }

    fn permute(v: &mut Vec<usize>, k: usize, f: &mut impl FnMut(&[usize])) {
        if k == v.len() {
            f(v);
            return;
        }
        for i in k..v.len() {
            v.swap(k, i);
            permute(v, k + 1, f);
            v.swap(k, i);
        }
    }

    fn valid_orders(lib: &Library, books: &[usize]) -> Vec<Vec<usize>> {
        fn go(
            lib: &Library,
            left: &mut Vec<usize>,
            cur: &mut Vec<usize>,
            out: &mut Vec<Vec<usize>>,
        ) {
            if left.is_empty() {
                out.push(cur.clone());
                return;
            }
            for i in 0..left.len() {
                let b = left[i];
                if left.iter().any(|&a| lib.precedes(a, b)) {
                    continue;
                }
                left.remove(i);
                cur.push(b);
                go(lib, left, cur, out);
                cur.pop();
                left.insert(i, b);
            }
        }
        let mut out = Vec::new();
        go(lib, &mut books.to_vec(), &mut Vec::new(), &mut out);
        out
    }

    fn brute_force(lib: &Library, k0: &Known, pr: &Params, books: &[usize]) -> Path {
        fn perms(v: &mut Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>) {
            if k == v.len() {
                out.push(v.clone());
                return;
            }
            for i in k..v.len() {
                v.swap(k, i);
                perms(v, k + 1, out);
                v.swap(k, i);
            }
        }
        let mut all = Vec::new();
        perms(&mut books.to_vec(), 0, &mut all);
        all.into_iter()
            .map(|o| lib.evaluate(k0, pr, &o))
            .min_by_key(|p| p.key())
            .unwrap()
    }

    /// Random libraries of six books: the beam is never worse than greedy,
    /// and close to trying every order.
    #[test]
    fn beam_against_every_order() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        let pool: Vec<String> = (0..60)
            .map(|i| char::from_u32(0x4e00 + i).unwrap().to_string())
            .collect();
        let (mut optimal, mut runs) = (0, 0);
        let (mut worst, mut gaps) = (0f64, 0f64);
        for round in 0..30 {
            let books: Vec<(String, Profile)> = (0..6)
                .map(|b| {
                    let mut m: HashMap<&str, u32> = HashMap::new();
                    for _ in 0..25 {
                        // Zipf-ish counts over a part of the pool.
                        let w = &pool[(rnd(40) + b * 4) as usize % pool.len()];
                        *m.entry(w).or_default() += 1 + (60 / (1 + rnd(30))) as u32;
                    }
                    let v: Vec<(&str, u32)> = m.into_iter().collect();
                    // Titles with no volume number: no series.
                    profile(["p", "q", "r", "s", "t", "u"][b as usize], &v, &[])
                })
                .collect();
            let lib = Library::build(false, false, books);
            let k0 = lib.known(None);
            let idx: Vec<usize> = (0..6).collect();
            for (study, objective) in [
                (Study::Target(90.0), Objective::Peak),
                (Study::Fixed(3), Objective::Peak),
                (Study::Target(90.0), Objective::Total),
                (Study::Fixed(3), Objective::Total),
                (READ, Objective::Peak),
                (READ, Objective::Total),
            ] {
                let mut pr = params(study, if round % 2 == 0 { 0.0 } else { 0.5 });
                pr.objective = objective;
                let none = AtomicBool::new(false);
                let greedy = lib.optimise(&k0, &pr, &idx, 1, &none).unwrap();
                let beam = lib.optimise(&k0, &pr, &idx, 16, &none).unwrap();
                let brute = brute_force(&lib, &k0, &pr, &idx);
                assert!(beam.key() <= greedy.key(), "beam worse than greedy");
                assert!(brute.key() <= beam.key());
                runs += 1;
                optimal += usize::from(beam.key() == brute.key());
                let (b, x) = (beam.key()[0] as f64, brute.key()[0] as f64);
                let gap = (b - x).abs() / x.abs().max(1.0);
                worst = worst.max(gap);
                gaps += gap;
            }
        }
        // Step costs here are a few words, so one word is a gap of 10% or more.
        let mean = gaps / runs as f64;
        eprintln!(
            "beam optimal in {optimal} of {runs}; gap mean {:.2}%, worst {:.1}%",
            mean * 100.0,
            worst * 100.0
        );
        assert!(
            optimal * 10 >= runs * 7,
            "beam optimal in only {optimal} of {runs}"
        );
        assert!(
            mean < 0.02,
            "beam {:.2}% from the best order on average",
            mean * 100.0
        );
    }
}
