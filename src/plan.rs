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
//! **Writing style.** With ratings on ([`crate::style`]), a book whose prose
//! is harder than typical gets a higher target (or, under a fixed number,
//! counts as read at a lower coverage), and an easier one a lower:
//! [`LibBook::offset`] percentage points.

use crate::anki::Knowledge;
use crate::lang::Language;
use crate::profile::Profile;
use crate::series;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

pub mod stats;

/// How much to study before each book.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Study {
    /// Until this percentage of the book's running words is known.
    Target(f64),
    /// This many of its unknown words.
    Fixed(usize),
}

/// What makes one order better than another.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
}

impl Default for Params {
    fn default() -> Self {
        Params {
            study: Study::Target(95.0),
            objective: Objective::default(),
            incidental: None,
            lookahead: DEFAULT_LOOKAHEAD,
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
    /// History ids of the books left out of the path.
    pub left_out: Vec<String>,
    /// Reading orders set by hand (History ids), by language name; a
    /// language without one is ordered by the planner.
    pub manual: HashMap<String, Vec<String>>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            fixed: false,
            target: 95.0,
            learn: 300,
            incidental_on: false,
            incidental: 10,
            lookahead: DEFAULT_LOOKAHEAD,
            read_percent: 90,
            read_min_count: 10,
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
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)
                .map_err(|e| format!("Cannot create {}: {e}", d.display()))?;
        }
        std::fs::write(&p, serde_json::to_string_pretty(self).unwrap())
            .map_err(|e| format!("Cannot write {}: {e}", p.display()))
    }

    pub fn params(&self) -> Params {
        Params {
            study: if self.fixed {
                Study::Fixed(self.learn)
            } else {
                Study::Target(self.target)
            },
            objective: Objective::default(),
            incidental: self.incidental_on.then_some(self.incidental),
            lookahead: self.lookahead,
        }
    }

    /// The hand-set order for `lang`, if any.
    pub fn manual_order(&self, lang: Language) -> Option<&Vec<String>> {
        self.manual.get(lang.name())
    }
}

/// A set of word ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Known(Vec<u64>);

impl Known {
    fn new(n: usize) -> Known {
        Known(vec![0; n.div_ceil(64)])
    }
    #[inline]
    pub fn has(&self, w: u32) -> bool {
        self.0[(w >> 6) as usize] >> (w & 63) & 1 == 1
    }
    #[inline]
    fn add(&mut self, w: u32) {
        self.0[(w >> 6) as usize] |= 1 << (w & 63);
    }
    pub fn len(&self) -> usize {
        self.0.iter().map(|x| x.count_ones() as usize).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|x| *x == 0)
    }
}

/// One book of the library, its words numbered across the library.
#[derive(Clone, Debug)]
pub struct LibBook {
    pub id: String,
    pub title: String,
    pub segmenter: String,
    pub variant: String,
    /// (word, count), most frequent first.
    words: Vec<(u32, u32)>,
    /// Parallel to `words`: the word contains a name.
    name: Vec<bool>,
    /// Japanese: a long unit counts as known when all its parts are.
    parts: FxHashMap<u32, Vec<u32>>,
    /// Running words that are not names: what shares are taken of.
    denom: u64,
    /// The book's own spelling of a word, where it differs from the key.
    spell: FxHashMap<u32, String>,
    /// The series it belongs to (numbered within the library) and its
    /// volume ([`series::Volume::number`]), if another book of the library
    /// is in that series.
    pub series: Option<(u32, Vec<u32>)>,
    /// Percentage points added to the target for this book (taken off its
    /// reading coverage under a fixed number), from its writing style
    /// ([`crate::style::target_offset`]); 0 by default.
    pub offset: f64,
}

impl LibBook {
    pub fn unique_words(&self) -> usize {
        self.words.len()
    }
    pub fn running_words(&self) -> u64 {
        self.words.iter().map(|w| w.1 as u64).sum()
    }
}

/// The books of one language, with every word numbered once (after
/// Traditional → Simplified folding when `fold`).
#[derive(Clone, Debug)]
pub struct Library {
    pub language: Language,
    pub fold: bool,
    vocab: Vec<String>,
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
    /// Words learned from reading it ([`Params::incidental`]).
    pub by_reading: usize,
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

fn add_cost(k: Key, c: i64) -> Key {
    [k[0].max(c), k[1] + c]
}

/// The accumulated (largest, sum) in the order `objective` compares them.
fn order_key(objective: Objective, k: Key) -> Key {
    match objective {
        Objective::Peak => k,
        Objective::Total => [k[1], k[0]],
    }
}

/// Words studied, or under a fixed number, the reading coverage in
/// hundredths of a percent, negated (so that smaller is better).
fn step_cost(study: Study, s: &Step) -> i64 {
    match study {
        Study::Target(_) => s.study.len() as i64,
        Study::Fixed(_) => -((s.reading - s.offset) * 100.0).round() as i64,
    }
}

/// A partial path in the search.
#[derive(Clone)]
struct State {
    steps: Vec<Step>,
    known: Known,
    /// Each word's share summed over the books still to come (lookahead only).
    future: Vec<f32>,
    /// Books on the path, as a bit set over the library.
    done: Vec<u64>,
    key: Key,
}

impl Library {
    /// `books`: (title, profile) of one language. `fold` must be the
    /// knowledge's (see [`Library::fold_for`]). `skip_hapax`: leave out a
    /// book's words that occur only once in it, as the Overview does.
    pub fn build(
        language: Language,
        fold: bool,
        skip_hapax: bool,
        books: Vec<(String, Profile)>,
    ) -> Library {
        let mut ids: FxHashMap<String, u32> = FxHashMap::default();
        let mut vocab: Vec<String> = Vec::new();
        let mut intern = |w: &str| -> (u32, bool) {
            let k = crate::anki::fold(w, fold);
            let same = k == w;
            if let Some(&i) = ids.get(&k) {
                return (i, same);
            }
            let i = vocab.len() as u32;
            vocab.push(k.clone());
            ids.insert(k, i);
            (i, same)
        };
        let mut out = Vec::new();
        for (title, p) in books.into_iter().filter(|(_, p)| p.language == language) {
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
            let mut spell: FxHashMap<u32, String> = FxHashMap::default();
            let mut parts: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
            // Before folding, so the same words go as on the Overview.
            for (w, n) in p.words.iter().filter(|(_, n)| !skip_hapax || *n > 1) {
                let (id, unfolded) = intern(w);
                let is_name = ac.as_ref().is_some_and(|a| a.is_match(w.as_str()));
                let e = counts.entry(id).or_insert((0, false));
                e.0 += n;
                e.1 |= is_name;
                // The most frequent spelling of a key in this book is how it is shown.
                let shown = p.shown(w);
                if !spell.contains_key(&id) && (!unfolded || shown != w) {
                    spell.insert(id, shown.to_owned());
                }
                if let Some(ps) = p.parts.get(w).filter(|ps| !ps.is_empty()) {
                    parts.insert(id, ps.iter().map(|x| intern(x).0).collect());
                }
            }
            let mut words: Vec<(u32, u32, bool)> =
                counts.into_iter().map(|(w, (n, nm))| (w, n, nm)).collect();
            words.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let denom = words.iter().filter(|w| !w.2).map(|w| w.1 as u64).sum();
            out.push(LibBook {
                id: p.id,
                title,
                segmenter: p.segmenter,
                variant: p.variant,
                name: words.iter().map(|w| w.2).collect(),
                words: words.iter().map(|w| (w.0, w.1)).collect(),
                parts,
                denom,
                spell,
                series: None,
                offset: 0.0,
            });
        }
        // Series: two or more books whose titles name one, at least one with
        // a volume number.
        let vols: Vec<series::Volume> = out.iter().map(|b| series::parse(&b.title)).collect();
        let mut groups: FxHashMap<&str, Vec<usize>> = FxHashMap::default();
        for (i, v) in vols.iter().enumerate() {
            if !v.key.is_empty() {
                groups.entry(v.key.as_str()).or_default().push(i);
            }
        }
        let mut groups: Vec<Vec<usize>> = groups
            .into_values()
            .filter(|m| m.len() > 1 && m.iter().any(|&i| vols[i].number.is_some()))
            .collect();
        groups.sort_unstable();
        for (id, members) in groups.into_iter().enumerate() {
            for i in members {
                let n = vols[i]
                    .number
                    .clone()
                    .unwrap_or_else(|| vec![series::UNNUMBERED]);
                out[i].series = Some((id as u32, n));
            }
        }
        Library {
            language,
            fold,
            vocab,
            books: out,
        }
    }

    /// The folding a library must be built with to match `k`.
    pub fn fold_for(k: Option<&Knowledge>, language: Language, match_across_scripts: bool) -> bool {
        match k {
            Some(k) => k.fold,
            None => match_across_scripts && language == Language::Chinese,
        }
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

    fn is_known(&self, b: &LibBook, w: u32, k: &Known) -> bool {
        k.has(w) || b.parts.get(&w).is_some_and(|p| p.iter().all(|&x| k.has(x)))
    }

    fn scan(&self, book: usize, k: &Known) -> Scan {
        let b = &self.books[book];
        let mut s = Scan {
            known: 0,
            total: 0,
            unknown: Vec::new(),
        };
        for (&(w, n), &name) in b.words.iter().zip(&b.name) {
            if self.is_known(b, w, k) {
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
    fn cheap(&self, book: usize, k: &Known, study: Study) -> (f64, i64) {
        let b = &self.books[book];
        let (mut known, mut total) = (0u64, 0u64);
        for (&(w, n), &name) in b.words.iter().zip(&b.name) {
            if self.is_known(b, w, k) {
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
            .filter(|(w, name)| !**name && !self.is_known(b, w.0, k))
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
                (now, n)
            }
            Study::Fixed(n) => {
                let add: u64 = unknown.take(n).sum();
                (now, -((pct(known + add) - b.offset) * 100.0).round() as i64)
            }
        }
    }

    /// The step for `book` from state `k` (and `future`, with lookahead).
    fn step(
        &self,
        book: usize,
        k: &mut Known,
        future: &mut [f32],
        params: &Params,
        known0: &Known,
    ) -> Step {
        let b = &self.books[book];
        let s = self.scan(book, k);
        let reached = s.percent();
        let now_scan = self.scan(book, known0);
        let now = now_scan.percent();
        let gained = now_scan.unknown.iter().filter(|(w, _)| k.has(*w)).count();
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
            Study::Target(_) => Some(self.cheap(book, known0, params.study).1 as usize),
            Study::Fixed(_) => None,
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
        let mut k = known0.clone();
        let mut future = if params.lookahead > 0.0 {
            self.future(order)
        } else {
            Vec::new()
        };
        let steps = order
            .iter()
            .map(|&b| self.step(b, &mut k, &mut future, params, known0))
            .collect();
        Path {
            steps,
            params: *params,
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
    /// volume order. `None` if cancelled.
    pub fn optimise(
        &self,
        known0: &Known,
        params: &Params,
        books: &[usize],
        width: usize,
        cancel: &AtomicBool,
    ) -> Option<Path> {
        let mut found = Vec::new();
        if width > 1 {
            found.push(self.beam(known0, params, books, width, cancel)?);
        }
        found.push(self.beam(known0, params, books, 1, cancel)?);
        found.push(self.evaluate(known0, params, &self.easiest_now(known0, books)));
        // The first of the best: the beam's when it ties.
        found.into_iter().min_by_key(|p| p.key())
    }

    fn beam(
        &self,
        known0: &Known,
        params: &Params,
        books: &[usize],
        width: usize,
        cancel: &AtomicBool,
    ) -> Option<Path> {
        let width = width.max(1);
        let words = (self.books.len()).div_ceil(64);
        let mut beam = vec![State {
            steps: Vec::new(),
            known: known0.clone(),
            future: if params.lookahead > 0.0 {
                self.future(books)
            } else {
                Vec::new()
            },
            done: vec![0; words],
            key: EMPTY,
        }];
        let in_done = |d: &[u64], b: usize| d[b / 64] >> (b % 64) & 1 == 1;
        let earlier = self.earlier_volumes(books);
        for _ in 0..books.len() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            // Every (state, next book), ranked by the cheap cost. A book of a
            // series waits for its earlier volumes; one of them is always
            // free, since volumes are ordered.
            let mut cands: Vec<(Key, usize, usize)> = beam
                .par_iter()
                .enumerate()
                .flat_map_iter(|(si, s)| {
                    let earlier = &earlier;
                    books
                        .iter()
                        .filter(move |&&b| {
                            !in_done(&s.done, b) && earlier[b].iter().all(|&a| in_done(&s.done, a))
                        })
                        .map(move |&b| (si, b))
                })
                .map(|(si, b)| {
                    let s = &beam[si];
                    let (_, c) = self.cheap(b, &s.known, params.study);
                    (order_key(params.objective, add_cost(s.key, c)), si, b)
                })
                .collect();
            cands.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
            // The most promising distinct sets of books, made real.
            let mut seen: FxHashSet<Vec<u64>> = FxHashSet::default();
            let picked: Vec<(usize, usize)> = cands
                .iter()
                .filter(|(_, si, b)| {
                    let mut d = beam[*si].done.clone();
                    d[b / 64] |= 1 << (b % 64);
                    seen.insert(d)
                })
                .take(width * 2)
                .map(|(_, si, b)| (*si, *b))
                .collect();
            let mut next: Vec<State> = picked
                .par_iter()
                .map(|&(si, b)| {
                    let mut s = beam[si].clone();
                    let st = self.step(b, &mut s.known, &mut s.future, params, known0);
                    s.key = add_cost(s.key, step_cost(params.study, &st));
                    s.done[b / 64] |= 1 << (b % 64);
                    s.steps.push(st);
                    s
                })
                .collect();
            next.sort_by_key(|s| order_key(params.objective, s.key));
            next.truncate(width);
            beam = next;
        }
        beam.into_iter().next().map(|s| Path {
            steps: s.steps,
            params: *params,
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
    let avg = books
        .iter()
        .map(|&b| lib.books[b].words.len() as f64)
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
                language: Language::Chinese,
                variant: "Simplified".into(),
                segmenter: "LTP".into(),
                words: w,
                names: names.iter().map(|s| s.to_string()).collect(),
                parts: HashMap::new(),
                display: HashMap::new(),
            },
        )
    }

    fn params(study: Study, lookahead: f32) -> Params {
        Params {
            study,
            objective: Objective::Peak,
            incidental: None,
            lookahead,
        }
    }

    #[test]
    fn target_takes_the_fewest_frequent_words() {
        let lib = Library::build(
            Language::Chinese,
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
            Language::Chinese,
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
            Language::Chinese,
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
        let all = Library::build(Language::Chinese, false, false, books());
        assert_eq!(
            (all.books[0].unique_words(), all.books[0].running_words()),
            (3, 10)
        );
        let lib = Library::build(Language::Chinese, false, true, books());
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
            Language::Chinese,
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

    #[test]
    fn small_library_matches_every_order() {
        // B shares its vocabulary with A and C: whichever comes first makes the others easier.
        let a = profile("a", &[("一", 40), ("二", 30), ("三", 30)], &[]);
        let b = profile("b", &[("一", 25), ("二", 25), ("四", 25), ("五", 25)], &[]);
        let c = profile("c", &[("四", 40), ("五", 30), ("六", 30)], &[]);
        let lib = Library::build(Language::Chinese, false, false, vec![a, b, c]);
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
        let lib = Library::build(Language::Chinese, false, false, vec![a, b]);
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
        let lib = Library::build(Language::Chinese, false, false, vec![a, b]);
        let k0 = lib.known(None);
        let mut pr = params(Study::Fixed(0), 0.0);
        pr.incidental = Some(10);
        let p = lib.evaluate(&k0, &pr, &[0, 1]);
        assert_eq!(p.steps[0].by_reading, 1);
        assert_eq!(p.steps[1].reached, 50.0);
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
        let lib = Library::build(Language::Chinese, false, false, books);
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
            let lib = Library::build(Language::Chinese, false, false, books);
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
            let lib = Library::build(Language::Chinese, false, false, books);
            let k0 = lib.known(None);
            let idx: Vec<usize> = (0..6).collect();
            for (study, objective) in [
                (Study::Target(90.0), Objective::Peak),
                (Study::Fixed(3), Objective::Peak),
                (Study::Target(90.0), Objective::Total),
                (Study::Fixed(3), Objective::Total),
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
