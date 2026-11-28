//! Books marked read in the Library. Marking one counts the most frequent of
//! its words as known ([`known::ReadBook`], [`known::read_selection`]), takes
//! it off the reading path, and lists it in its own section of Library Stats.
//!
//! The marks live in the known-words file, with the words they added, so
//! unmarking a book takes back exactly those.

use crate::icons::{self, CHECK, GEAR, WARNING};
use crate::library_stats::{Tile, fmt_time, reading_minutes, tile_grid};
use crate::{AnkiState, App, fmt_int};
use eframe::egui::{self, RichText};
use hanstat::anki::Knowledge;
use hanstat::history;
use hanstat::known::{self, ReadBook};
use hanstat::profile::Profile;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// New words listed in the dialog, most frequent first.
const SHOW_NEW: usize = 80;

/// Word counts of a book read, as frozen when it was marked.
type Counts = Arc<Vec<(String, u32)>>;

/// The words the books read teach together ([`known::across_books`]), kept
/// in `known.json` as [`known::Settings::across`] and worked out again in the
/// background whenever the marks, the reading rule or the T/S matching change.
#[derive(Default)]
pub(crate) struct Across {
    /// What the last result was worked out from.
    key: Option<u64>,
    job: Option<(u64, Receiver<AcrossDone>)>,
    /// Word counts of the books read, by History id (their frozen copies).
    pub(crate) counts: Arc<HashMap<String, Counts>>,
    /// Books marked read with no word counts: marked before copies were
    /// kept, and no longer in the Library.
    pub(crate) missing: usize,
}

struct AcrossDone {
    counts: HashMap<String, Counts>,
    missing: usize,
    words: Vec<String>,
}

/// Of `marks`, those with word counts, as [`known::across_books`] takes them.
pub(crate) fn across_input<'a>(
    marks: &'a [ReadBook],
    counts: &'a HashMap<String, Counts>,
) -> Vec<known::ReadCounts<'a>> {
    marks
        .iter()
        .filter_map(|m| counts.get(&m.id).map(|c| (&c[..], &m.words[..])))
        .collect()
}

/// The Mark read window, for one book.
pub(crate) struct ReadDialog {
    id: String,
    title: String,
    /// The book's word counts, most frequent first (its profile).
    words: Arc<Vec<(String, u32)>>,
    running: u64,
    percent: u8,
    min_count: u32,
    /// What is known without this book's mark; `None`: nothing.
    before: Option<Arc<Knowledge>>,
    /// When it was marked read, if it is: the window edits that mark.
    marked: Option<u64>,
    /// Why "not known yet" may count too many.
    note: Option<String>,
    /// The other books read, as [`known::across_books`] takes them.
    others: Arc<Vec<(Counts, Vec<String>)>>,
    /// The words they teach together now, and what this mark would add:
    /// worked out for (percent, min_count).
    across_now: Arc<HashSet<String>>,
    across_new: Option<((u8, u32), usize)>,
    /// Working that out in the background, for (percent, min_count): it
    /// takes a while with many books read, so one at a time, for the rule
    /// as it is when the last one finishes (not every step of a drag).
    across_job: Option<((u8, u32), Receiver<usize>)>,
    /// Traditional and Simplified spellings count as one word.
    fold: bool,
    /// When it was read (day numbers), if given.
    started: Option<i32>,
    finished: Option<i32>,
    /// What 📆 Set picks for each: the first and last day read in the
    /// reader, or today.
    reader_days: (i32, i32),
    /// The book's Chinese characters.
    han: u64,
    /// Cached selection summary for `(percent, min_count)` so we don't re-sort
    /// 10k-40k words on every frame while the dialog is open.
    sel_cache: Option<SelCache>,
}

struct SelCache {
    rule: (u8, u32),
    sel_len: usize,
    often: usize,
    new_len: usize,
    new_sample: Vec<(String, u32)>,
    share: f64,
}

/// The dates picked in the Mark read window, or what is wrong with them.
fn check_dates(started: Option<i32>, finished: Option<i32>, today: i32) -> Result<(), String> {
    if started.max(finished).is_some_and(|d| d > today) {
        return Err("A date is after today.".to_owned());
    }
    if let (Some(a), Some(b)) = (started, finished)
        && b < a
    {
        return Err("The book is finished before it is started.".to_owned());
    }
    Ok(())
}

/// Anki (if on) plus the levels, word lists, marked words and books read:
/// what books are compared with.
pub(crate) fn merged_knowledge(anki: &AnkiState, k: &known::Settings) -> Option<Arc<Knowledge>> {
    let base = anki.knowledge.as_deref().filter(|_| anki.settings.enabled);
    if k.is_empty() {
        anki.knowledge.clone().filter(|_| anki.settings.enabled)
    } else {
        k.apply(base, anki.settings.match_across_scripts)
            .map(Arc::new)
    }
}

impl App {
    /// Every book marked read, by History id, with when it was marked.
    pub(crate) fn read_dates(&self) -> HashMap<String, u64> {
        self.known
            .settings
            .read
            .iter()
            .map(|b| (b.id.clone(), b.read))
            .collect()
    }

    /// Change the known words with `f` (which says whether it changed
    /// anything), save them, and bring what depends on them up to date.
    pub(crate) fn edit_known(
        &mut self,
        ctx: &egui::Context,
        f: impl FnOnce(&mut known::Settings) -> bool,
    ) {
        if f(&mut self.known.settings) {
            self.known_changed(ctx);
            if let Some(e) = &self.known.error {
                self.history_error = Some(e.clone());
            }
        }
    }

    /// Open the Mark read window for Library book `id`.
    pub(crate) fn open_read_dialog(&mut self, id: &str) {
        let Some(e) = self.history.entries.iter().find(|e| e.id == id) else {
            return;
        };
        let title = e.title.clone();
        let Some(p) = Profile::load(&self.history.profile(id)) else {
            self.history_error = Some(format!(
                "{title} has no word data yet: open it (or recalculate the Library) first."
            ));
            return;
        };
        let mut k = self.known.settings.clone();
        let mark = k.read_book(id).cloned();
        k.unset_read(id);
        let before = merged_knowledge(&self.anki, &k);
        let note = (self.anki.rx.is_some() || self.anki.dirty).then(|| {
            "Anki is still being read: words known there may be counted as not known yet."
                .to_owned()
        });
        let running = p.words.iter().map(|(_, n)| *n as u64).sum();
        let reader_days = {
            use crate::reading_stats as rs;
            let now = rs::now();
            rs::with(|s| {
                let today = rs::local_day(now, s.settings.day_start_hour);
                let days = s.books.get(id).map(|b| {
                    let mut read = b.days.iter().filter(|(_, d)| d.read_chars > 0);
                    let first = read.next().map(|(k, _)| *k);
                    (first, read.next_back().map(|(k, _)| *k).or(first))
                });
                match days {
                    Some((Some(a), Some(b))) => (a, b),
                    _ => (today, today),
                }
            })
        };
        let others: Vec<(Counts, Vec<String>)> = k
            .read
            .iter()
            .filter_map(|m| {
                self.across
                    .counts
                    .get(&m.id)
                    .map(|c| (c.clone(), m.words.clone()))
            })
            .collect();
        let across_now: HashSet<String> = self.known.settings.across.iter().cloned().collect();
        self.read_dialog = Some(ReadDialog {
            id: id.to_owned(),
            title,
            words: Arc::new(p.words),
            running,
            percent: mark
                .as_ref()
                .map_or(self.plan.settings.read_percent, |m| m.percent),
            min_count: mark
                .as_ref()
                .map_or(self.plan.settings.read_min_count, |m| m.min_count),
            before,
            marked: mark.as_ref().map(|m| m.read),
            note,
            others: Arc::new(others),
            across_now: Arc::new(across_now),
            across_new: None,
            across_job: None,
            fold: self.anki.settings.match_across_scripts,
            started: mark.as_ref().and_then(|m| m.started),
            finished: mark.as_ref().and_then(|m| m.finished),
            reader_days,
            han: e.stats.han_total,
            sel_cache: None,
        });
    }

    /// The Mark read window, while open.
    pub(crate) fn read_window(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.read_dialog else {
            return;
        };
        let mut open = true;
        let mut act: Option<ReadAct> = None;
        let max_inner = crate::palette::popup_max_inner(ctx, crate::palette::popup_bounds(ctx));
        crate::palette::popup_window(ctx, format!("Mark read: {}", d.title))
            .id(egui::Id::unique("read_dialog"))
            .open(&mut open)
            .collapsible(false)
            .default_width(520.0_f32.min(max_inner.x))
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Count as known the");
                    ui.add(
                        egui::DragValue::new(&mut d.percent)
                            .range(1..=100)
                            .suffix("%"),
                    );
                    ui.label("most frequent of the words it uses at least");
                    ui.add(
                        egui::DragValue::new(&mut d.min_count)
                            .range(1..=10_000)
                            .speed(0.2),
                    );
                    ui.label("times.");
                });
                ui.add_space(4.0);
                let rule_key = (d.percent, d.min_count);
                if d.sel_cache.as_ref().is_none_or(|c| c.rule != rule_key) {
                    let sel = known::read_selection(&d.words, d.percent, d.min_count);
                    let often = d.words.iter().filter(|(_, n)| *n >= d.min_count).count();
                    let known_fn =
                        |w: &str| d.before.as_ref().is_some_and(|k| k.word(w).is_known());
                    let mut new_len = 0usize;
                    let mut new_sample = Vec::with_capacity(SHOW_NEW);
                    for &(w, n) in &sel {
                        if !known_fn(w) {
                            if new_sample.len() < SHOW_NEW {
                                new_sample.push((w.clone(), *n));
                            }
                            new_len += 1;
                        }
                    }
                    let share = if d.running == 0 {
                        0.0
                    } else {
                        sel.iter().map(|(_, n)| *n as u64).sum::<u64>() as f64 * 100.0
                            / d.running as f64
                    };
                    d.sel_cache = Some(SelCache {
                        rule: rule_key,
                        sel_len: sel.len(),
                        often,
                        new_len,
                        new_sample,
                        share,
                    });
                }
                let sc = d.sel_cache.as_ref().unwrap();
                ui.label(format!(
                    "{} of the {} words it uses {}+ times; {} of them not known yet. Together they are {:.1}% of its running words.",
                    fmt_int(sc.sel_len as u64),
                    fmt_int(sc.often as u64),
                    d.min_count,
                    fmt_int(sc.new_len as u64),
                    sc.share,
                ));
                // What marking it adds across the books read (other books
                // with these words): worked out again when the rule changes.
                if !d.others.is_empty() {
                    let rule = (d.percent, d.min_count);
                    if let Some((r, rx)) = &d.across_job {
                        match rx.try_recv() {
                            Ok(n) => {
                                d.across_new = Some((*r, n));
                                d.across_job = None;
                            }
                            Err(TryRecvError::Empty) => {}
                            Err(TryRecvError::Disconnected) => d.across_job = None,
                        }
                    }
                    if d.across_job.is_none() && d.across_new.is_none_or(|(r, _)| r != rule) {
                        d.across_job = Some((rule, d.across_count(rule, ui.ctx())));
                    }
                    let working = d.across_new.is_none_or(|(r, _)| r != rule);
                    let hover = "A word none of the books took counts as known once it occurs in two or more books read, \
                                 this many times in all. The reading path's Learn while reading mode counts the same way.";
                    ui.horizontal_wrapped(|ui| {
                        if working {
                            crate::palette::spinner(ui);
                        }
                        match d.across_new {
                            // The last figure, for the rule it was worked out for.
                            Some((r, n)) => ui.label(format!(
                                "With the {} other books read: {} more words met {}+ times across books.",
                                fmt_int(d.others.len() as u64),
                                fmt_int(n as u64),
                                r.1,
                            )),
                            None => ui.label(format!(
                                "With the {} other books read: counting the words met {}+ times across books…",
                                fmt_int(d.others.len() as u64),
                                d.min_count,
                            )),
                        }
                        .on_hover_text(hover);
                    });
                }
                if let Some(n) = &d.note {
                    ui.label(icons::rich(ui, RichText::new(format!("{WARNING} {n}")).color(crate::palette::warn())));
                }
                let sc = d.sel_cache.as_ref().unwrap();
                if sc.new_len > 0 {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(if sc.new_len > SHOW_NEW {
                            format!("Not known yet, most frequent first (the first {SHOW_NEW}):")
                        } else {
                            "Not known yet, most frequent first:".to_owned()
                        })
                        .weak(),
                    );
                    egui::ScrollArea::vertical()
                        .id_salt("read_new")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                for (w, n) in &sc.new_sample {
                                    ui.label(w.as_str())
                                        .on_hover_text(format!("{} times", fmt_int(*n as u64)));
                                }
                            });
                        });
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "These words count as known (\"Word list\") in every book, and the book leaves the reading path; \
                         it stays in the Library and its Stats. They don't change how books are segmented. \
                         Unmarking the book takes them back, except where something else marks them known.",
                    )
                    .weak()
                    .small(),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Read from");
                    let (a, b) = d.reader_days;
                    let tip_a = format!("{}; change it with the calendar", crate::reading_stats::long_date(a));
                    let tip_b = format!("{}; change it with the calendar", crate::reading_stats::long_date(b));
                    crate::library_books::day_picker_from(ui, &mut d.started, "read_started", a, &tip_a);
                    ui.label("to");
                    crate::library_books::day_picker_from(ui, &mut d.finished, "read_finished", b, &tip_b);
                    ui.label(RichText::new("(optional)").weak()).on_hover_text(
                        "When you read it, if not (only) in hanstat's reader: Library Stats' \
                         \"Characters read over time\" spreads the book's characters evenly over these days, \
                         less what the reader timed. A start alone runs to the day it was marked read; \
                         a finish alone is one day.",
                    );
                });
                let today = {
                    let now = crate::reading_stats::now();
                    crate::reading_stats::with(|s| {
                        crate::reading_stats::local_day(now, s.settings.day_start_hour)
                    })
                };
                let dates = check_dates(d.started, d.finished, today);
                if let Err(e) = &dates {
                    ui.colored_label(crate::palette::warn(), e);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let label = if d.marked.is_some() { "Save".to_owned() } else { format!("{CHECK} Mark read") };
                    if ui.add_enabled(dates.is_ok(), egui::Button::new(icons::rich(ui, label))).clicked()
                    {
                        let sel = known::read_selection(&d.words, d.percent, d.min_count);
                        let words: Vec<String> = sel.iter().map(|(w, _)| w.clone()).collect();
                        let added = d.sel_cache.as_ref().map_or(0, |c| c.new_len);
                        act = Some(ReadAct::Mark(ReadBook {
                            id: d.id.clone(),
                            title: d.title.clone(),
                            read: d.marked.unwrap_or_else(history::now),
                            percent: d.percent,
                            min_count: d.min_count,
                            words,
                            added,
                            started: d.started,
                            finished: d.finished,
                            han: d.han,
                        }));
                    }
                    if let Some(t) = d.marked {
                        if ui
                            .button("Unmark read")
                            .on_hover_text("Put it back on the reading path and take back its words")
                            .clicked()
                        {
                            act = Some(ReadAct::Unmark);
                        }
                        ui.label(
                            RichText::new(format!("Marked read {}", history::age(t, history::now())))
                                .weak(),
                        )
                        .on_hover_text(history::date_time(t));
                    }
                    if ui.button("Cancel").clicked() {
                        act = Some(ReadAct::Cancel);
                    }
                });
            });
        let (id, title) = (d.id.clone(), d.title.clone());
        let time = ctx.input(|i| i.time);
        match act {
            None if open => {}
            None | Some(ReadAct::Cancel) => self.read_dialog = None,
            Some(ReadAct::Mark(b)) => {
                self.read_dialog = None;
                if (b.percent, b.min_count)
                    != (
                        self.plan.settings.read_percent,
                        self.plan.settings.read_min_count,
                    )
                {
                    self.plan.settings.read_percent = b.percent;
                    self.plan.settings.read_min_count = b.min_count;
                    if let Err(e) = self.plan.settings.save() {
                        self.history_error = Some(e);
                    }
                }
                let msg = format!(
                    "Marked {title} read: {} words, {} of them new",
                    fmt_int(b.words.len() as u64),
                    fmt_int(b.added as u64)
                );
                self.edit_known(ctx, |k| {
                    let changed = k.read_book(&b.id) != Some(&b);
                    k.set_read(b);
                    changed
                });
                self.toast = Some((msg, time + 3.0));
            }
            Some(ReadAct::Unmark) => {
                self.read_dialog = None;
                self.edit_known(ctx, |k| k.unset_read(&id));
                // Marked again, it is copied again as it is then.
                let _ = std::fs::remove_file(hanstat::profile::read_copy_path_in(
                    &history::History::path(),
                    &id,
                ));
                self.toast = Some((format!("Unmarked {title}"), time + 3.0));
            }
        }
    }

    /// Keep [`known::Settings::across`] up to date with the books marked
    /// read, the reading rule ([`crate::plan::Settings::read_min_count`]) and
    /// the T/S matching: worked out in the background, then saved.
    pub(crate) fn poll_across(&mut self, ctx: &egui::Context) {
        use std::hash::{Hash, Hasher};
        let fold = self.anki.settings.match_across_scripts;
        let min_count = self.plan.settings.read_min_count;
        // Every frame: what tells marks apart, not their words.
        let key = {
            let mut h = std::hash::DefaultHasher::new();
            (fold, min_count).hash(&mut h);
            for m in &self.known.settings.read {
                (&m.id, m.read, m.percent, m.min_count, m.words.len()).hash(&mut h);
            }
            h.finish()
        };
        if self.across.key == Some(key) {
            return;
        }
        if let Some((k, rx)) = &self.across.job {
            let current = *k == key;
            let done = match rx.try_recv() {
                Ok(done) if current => done,
                Err(TryRecvError::Disconnected) if current => return,
                // One at a time: dragging the rule starts no thread per step.
                Err(TryRecvError::Empty) => return,
                // Worked out for marks or a rule changed since: again, below.
                Ok(_) | Err(TryRecvError::Disconnected) => {
                    self.across.job = None;
                    return self.poll_across(ctx);
                }
            };
            self.across.job = None;
            self.across.key = Some(key);
            self.across.counts = Arc::new(done.counts);
            self.across.missing = done.missing;
            let words = done.words;
            self.edit_known(ctx, |k| {
                let changed = k.across != words;
                k.across = words;
                changed
            });
            return;
        }
        // Start (again, for what changed meanwhile).
        let marks = self.known.settings.read.clone();
        let had = self.across.counts.clone();
        let ctx = ctx.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let hist = history::History::path();
            let mut counts: HashMap<String, Counts> = HashMap::new();
            let mut missing = 0;
            for m in &marks {
                if let Some(c) = had.get(&m.id) {
                    counts.insert(m.id.clone(), c.clone());
                    continue;
                }
                let copy = hanstat::profile::read_copy_path_in(&hist, &m.id);
                // Marked before copies were kept (or just now): copied from
                // the Library's word data while it is there.
                if !copy.is_file()
                    && let Ok(b) = hanstat::saver::read(&hanstat::profile::path_in(&hist, &m.id))
                {
                    let _ = std::fs::create_dir_all(copy.parent().unwrap());
                    let _ = hanstat::saver::write(&copy, b);
                }
                match Profile::load(&copy) {
                    Some(p) => {
                        counts.insert(m.id.clone(), Arc::new(p.words));
                    }
                    None => missing += 1,
                }
            }
            let words = known::across_books(&across_input(&marks, &counts), min_count, fold);
            let _ = tx.send(AcrossDone {
                counts,
                missing,
                words,
            });
            ctx.request_repaint();
        });
        self.across.job = Some((key, rx));
    }

    /// Library Stats: the books marked read.
    pub(crate) fn read_section(&self, ui: &mut egui::Ui, entries: &[&history::Entry]) {
        let marks = self.known.settings.read.as_slice();
        ui.add_space(10.0);
        ui.label(RichText::new("Books read").size(16.0).strong());
        if marks.is_empty() {
            ui.label(icons::rich(
                ui,
                RichText::new(format!("None yet. Mark a book read with {CHECK} in the Read column of the Library's Books tab."))
                    .weak(),
            ));
            return;
        }
        let by_id: HashMap<&str, &history::Entry> =
            entries.iter().map(|e| (e.id.as_str(), *e)).collect();
        let in_lib: Vec<&history::Entry> = marks
            .iter()
            .filter_map(|m| by_id.get(m.id.as_str()).copied())
            .collect();
        let speeds = self.speeds();
        let len = |s: &history::Stats| s.han_total;
        let read_len: u64 = in_lib.iter().map(|e| len(&e.stats)).sum();
        let all_len: u64 = entries.iter().map(|e| len(&e.stats)).sum();
        let read_mins: f64 = in_lib
            .iter()
            .map(|e| reading_minutes(&e.stats, speeds))
            .sum();
        let distinct_len: usize = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            marks.len().hash(&mut h);
            for m in marks {
                m.id.hash(&mut h);
                m.read.hash(&mut h);
                m.words.len().hash(&mut h);
            }
            let key = h.finish();
            let id = egui::Id::unique("read_books_distinct_words");
            if let Some((k, n)) = ui.ctx().data(|d| d.get_temp::<(u64, usize)>(id))
                && k == key
            {
                n
            } else {
                let distinct: HashSet<&str> = marks
                    .iter()
                    .flat_map(|m| m.words.iter().map(String::as_str))
                    .collect();
                let n = distinct.len();
                ui.ctx().data_mut(|d| d.insert_temp(id, (key, n)));
                n
            }
        };
        let added: usize = marks.iter().map(|m| m.added).sum();
        let pct = |a: u64, b: u64| {
            if b == 0 {
                0.0
            } else {
                a as f64 * 100.0 / b as f64
            }
        };
        ui.add_space(4.0);
        tile_grid(
            ui,
            vec![
                Tile::new(
                    "Books",
                    RichText::new(fmt_int(marks.len() as u64)),
                    format!("of the {} books", fmt_int(entries.len() as u64)),
                ),
                Tile::new(
                    "Characters read",
                    RichText::new(fmt_int(read_len)),
                    format!("{:.1}% of the books' characters", pct(read_len, all_len)),
                )
                .hover("Of the books marked read that are still in the Library"),
                Tile::new(
                    "Time read",
                    RichText::new(fmt_time(read_mins)),
                    format!("at the speeds set in {GEAR}"),
                ),
                Tile::new(
                    "Words from them",
                    RichText::new(fmt_int(distinct_len as u64)),
                    format!("{} were new when marked", fmt_int(added as u64)),
                )
                .hover("Distinct words the books marked read count as known. A word new for two books is counted for each."),
                Tile::new(
                    "Across books",
                    RichText::new(fmt_int(self.known.settings.across.len() as u64)),
                    format!("met {}+ times in two or more", self.plan.settings.read_min_count),
                )
                .hover("Words none of the books took on its own that count as known because the books read use them, together, often enough"),
            ],
        );
        if self.across.missing > 0 {
            ui.label(
                RichText::new(format!(
                    "{} {} marked read before word counts were kept with the mark and no longer in the Library: {} words count, but not towards words across books.",
                    fmt_int(self.across.missing as u64),
                    if self.across.missing == 1 { "book was" } else { "books were" },
                    if self.across.missing == 1 { "its" } else { "their" },
                ))
                .weak(),
            );
        }
        ui.add_space(6.0);
        egui::Grid::new("read_books")
            .striped(true)
            .spacing([24.0, 4.0])
            .show(ui, |ui| {
                for h in ["Book", "Marked read", "Words taken", "Words", "New then"] {
                    ui.strong(h);
                }
                ui.end_row();
                let now = history::now();
                // Newest first.
                for m in marks.iter().rev() {
                    match by_id.get(m.id.as_str()) {
                        Some(e) => ui.label(&e.title),
                        None => ui
                            .label(RichText::new(&m.title).weak())
                            .on_hover_text("No longer in the Library; its words still count as known (unmark it in the Known words window)"),
                    };
                    ui.label(history::age(m.read, now))
                        .on_hover_text(history::date_time(m.read));
                    ui.label(format!("{}% of {}+ times", m.percent, m.min_count));
                    ui.label(fmt_int(m.words.len() as u64));
                    ui.label(fmt_int(m.added as u64));
                    ui.end_row();
                }
            });
    }
}

impl ReadDialog {
    /// Start working out, in the background, how many more words marking
    /// the book under `rule` (percent, min_count) adds across the books
    /// read: words none of them takes, met `min_count`+ times in two or more,
    /// not counted across books now and not known.
    fn across_count(&self, rule: (u8, u32), ctx: &egui::Context) -> Receiver<usize> {
        let (percent, min_count) = rule;
        let (words, others, now) = (
            self.words.clone(),
            self.others.clone(),
            self.across_now.clone(),
        );
        let (before, fold) = (self.before.clone(), self.fold);
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let taken: Vec<String> = known::read_selection(&words, percent, min_count)
                .iter()
                .map(|(w, _)| w.clone())
                .collect();
            let mut books: Vec<known::ReadCounts> =
                others.iter().map(|(c, t)| (&c[..], &t[..])).collect();
            books.push((&words[..], &taken[..]));
            let known = |w: &str| before.as_ref().is_some_and(|k| k.word(w).is_known());
            let n = known::across_books(&books, min_count, fold)
                .into_iter()
                .filter(|w| !now.contains(w) && !known(w))
                .count();
            let _ = tx.send(n);
            ctx.request_repaint();
        });
        rx
    }
}

enum ReadAct {
    Mark(ReadBook),
    Unmark,
    Cancel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_checked() {
        assert!(check_dates(None, None, 100).is_ok());
        assert!(check_dates(Some(90), Some(100), 100).is_ok());
        assert!(check_dates(Some(90), None, 100).is_ok());
        assert!(check_dates(Some(101), None, 100).is_err());
        assert!(check_dates(None, Some(101), 100).is_err());
        assert!(check_dates(Some(95), Some(90), 100).is_err());
    }
}
