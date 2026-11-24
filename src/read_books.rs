//! Books marked read in the Library. Marking one counts the most frequent of
//! its words as known ([`known::ReadBook`], [`known::read_selection`]), takes
//! it off the reading path, and lists it in its own section of Library Stats.
//!
//! The marks live in the known-words file, with the words they added, so
//! unmarking a book takes back exactly those.

use crate::library_stats::{Tile, fmt_time, reading_minutes, tile_grid};
use crate::{AnkiState, App, fmt_int};
use eframe::egui::{self, RichText};
use hanstat::anki::Knowledge;
use hanstat::history;
use hanstat::known::{self, ReadBook};
use hanstat::profile::Profile;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// New words listed in the dialog, most frequent first.
const SHOW_NEW: usize = 80;

/// The Mark read window, for one book.
pub(crate) struct ReadDialog {
    id: String,
    title: String,
    /// The book's word counts, most frequent first (its profile).
    words: Vec<(String, u32)>,
    running: u64,
    percent: u8,
    min_count: u32,
    /// What is known without this book's mark; `None`: nothing.
    before: Option<Arc<Knowledge>>,
    /// When it was marked read, if it is: the window edits that mark.
    marked: Option<u64>,
    /// Why "not known yet" may count too many.
    note: Option<String>,
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
    pub(crate) fn edit_known(&mut self, f: impl FnOnce(&mut known::Settings) -> bool) {
        if f(&mut self.known.settings) {
            self.known_changed();
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
        self.read_dialog = Some(ReadDialog {
            id: id.to_owned(),
            title,
            words: p.words,
            running,
            percent: mark
                .as_ref()
                .map_or(self.plan.settings.read_percent, |m| m.percent),
            min_count: mark
                .as_ref()
                .map_or(self.plan.settings.read_min_count, |m| m.min_count),
            before,
            marked: mark.map(|m| m.read),
            note,
        });
    }

    /// The Mark read window, while open.
    pub(crate) fn read_window(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.read_dialog else {
            return;
        };
        let mut open = true;
        let mut act: Option<ReadAct> = None;
        egui::Window::new(format!("Mark read: {}", d.title))
            .id(egui::Id::unique("read_dialog"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.set_max_width(520.0);
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
                let sel = known::read_selection(&d.words, d.percent, d.min_count);
                let often = d.words.iter().filter(|(_, n)| *n >= d.min_count).count();
                let known = |w: &str| d.before.as_ref().is_some_and(|k| k.word(w).is_known());
                let new: Vec<&(String, u32)> =
                    sel.iter().copied().filter(|(w, _)| !known(w)).collect();
                let share = if d.running == 0 {
                    0.0
                } else {
                    sel.iter().map(|(_, n)| *n as u64).sum::<u64>() as f64 * 100.0
                        / d.running as f64
                };
                ui.label(format!(
                    "{} of the {} words it uses {}+ times; {} of them not known yet. Together they are {share:.1}% of its running words.",
                    fmt_int(sel.len() as u64),
                    fmt_int(often as u64),
                    d.min_count,
                    fmt_int(new.len() as u64),
                ));
                if let Some(n) = &d.note {
                    ui.colored_label(crate::palette::warn(), format!("⚠ {n}"));
                }
                if !new.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(if new.len() > SHOW_NEW {
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
                                for (w, n) in new.iter().take(SHOW_NEW) {
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
                    let label = if d.marked.is_some() { "Save" } else { "✔ Mark read" };
                    if ui.button(label).clicked() {
                        let words: Vec<String> = sel.iter().map(|(w, _)| w.clone()).collect();
                        act = Some(ReadAct::Mark(ReadBook {
                            id: d.id.clone(),
                            title: d.title.clone(),
                            read: d.marked.unwrap_or_else(history::now),
                            percent: d.percent,
                            min_count: d.min_count,
                            words,
                            added: new.len(),
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
                self.edit_known(|k| {
                    let changed = k.read_book(&b.id) != Some(&b);
                    k.set_read(b);
                    changed
                });
                self.toast = Some((msg, time + 3.0));
            }
            Some(ReadAct::Unmark) => {
                self.read_dialog = None;
                self.edit_known(|k| k.unset_read(&id));
                self.toast = Some((format!("Unmarked {title}"), time + 3.0));
            }
        }
    }

    /// Library Stats: the books marked read.
    pub(crate) fn read_section(&self, ui: &mut egui::Ui, entries: &[&history::Entry]) {
        let marks = self.known.settings.read.as_slice();
        ui.add_space(10.0);
        ui.label(RichText::new("Books read").size(16.0).strong());
        if marks.is_empty() {
            ui.label(
                RichText::new("None yet. Mark a book read with ✔ in the Read column of the Library's Books tab.")
                    .weak(),
            );
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
        let distinct: HashSet<&str> = marks
            .iter()
            .flat_map(|m| m.words.iter().map(String::as_str))
            .collect();
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
                    "at the speeds set in ⚙".to_owned(),
                ),
                Tile::new(
                    "Words from them",
                    RichText::new(fmt_int(distinct.len() as u64)),
                    format!("{} were new when marked", fmt_int(added as u64)),
                )
                .hover("Distinct words the books marked read count as known. A word new for two books is counted for each."),
            ],
        );
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

enum ReadAct {
    Mark(ReadBook),
    Unmark,
    Cancel,
}
