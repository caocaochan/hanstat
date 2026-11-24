//! The Library's Stats tab: totals of every book's last analysis, then the
//! vocabulary of the books measured against what is known now, with charts.

use crate::charts::{self, Band, Bar, Point};
use crate::{App, LibKey, difficulty_color, difficulty_fill, fmt_int};
use eframe::egui::{self, Color32, RichText};
use hanstat::anki::{Difficulty, Knowledge};
use hanstat::history;
use hanstat::plan;
use hanstat::profile::Profile;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

const CHART_HEIGHT: f32 = 230.0;

pub(crate) fn reading_minutes(s: &history::Stats, cpm: f64) -> f64 {
    s.han_total as f64 / cpm
}

pub(crate) fn fmt_time(mins: f64) -> String {
    let h = (mins / 60.0) as u64;
    if h >= 100 {
        format!("{} h", fmt_int(h))
    } else {
        format!("{h}h {:02}m", (mins % 60.0) as u64)
    }
}

/// A book's length, in Chinese characters.
pub(crate) fn length(s: &history::Stats) -> u64 {
    s.han_total
}

pub(crate) fn pct(a: usize, b: usize) -> f64 {
    if b == 0 {
        0.0
    } else {
        a as f64 * 100.0 / b as f64
    }
}

/// A figure in a box: a title, a large value and a line under it.
pub(crate) struct Tile {
    title: &'static str,
    value: RichText,
    sub: String,
    hover: Option<String>,
}

impl Tile {
    pub(crate) fn new(title: &'static str, value: RichText, sub: String) -> Tile {
        Tile {
            title,
            value,
            sub,
            hover: None,
        }
    }

    pub(crate) fn hover(mut self, text: &str) -> Tile {
        self.hover = Some(text.to_owned());
        self
    }
}

/// Tiles in rows of as many equal columns as fit at least [`TILE_MIN`]
/// wide. A frame's size is only known once it is drawn, so a wrapping
/// layout can't move one to the next row: it runs off the right instead.
/// Text that doesn't fit is cut with "…" and shown whole on hover.
pub(crate) fn tile_grid(ui: &mut egui::Ui, tiles: Vec<Tile>) {
    const TILE_MIN: f32 = 210.0;
    const GAP: f32 = 8.0;
    const MARGIN: f32 = 12.0;
    if tiles.is_empty() {
        return;
    }
    let avail = ui.available_width();
    let cols = (((avail + GAP) / (TILE_MIN + GAP)).floor() as usize).clamp(1, tiles.len());
    let width = ((avail - GAP * (cols - 1) as f32) / cols as f32).floor();
    let stroke = ui.style().visuals.widgets.noninteractive.bg_stroke.width;
    let inner = (width - 2.0 * (MARGIN + stroke)).max(40.0);
    let mut tiles = tiles.into_iter().peekable();
    while tiles.peek().is_some() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for t in tiles.by_ref().take(cols) {
                let r = crate::palette::card()
                    .inner_margin(MARGIN)
                    .show(ui, |ui| {
                        ui.set_width(inner);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            let weak = crate::palette::weak();
                            let bold = egui::FontFamily::Name(crate::palette::BOLD.into());
                            ui.add(
                                egui::Label::new(RichText::new(t.title).size(12.5).color(weak))
                                    .truncate(),
                            );
                            ui.add_space(2.0);
                            ui.add(
                                egui::Label::new(t.value.size(30.0).family(bold).strong())
                                    .truncate(),
                            );
                            ui.add_space(2.0);
                            ui.add(egui::Label::new(RichText::new(t.sub).color(weak)).truncate());
                        });
                    })
                    .response;
                if let Some(h) = t.hover {
                    r.on_hover_text(h);
                }
            }
        });
        ui.add_space(GAP);
    }
}

/// A titled frame around a chart.
pub(crate) fn card(ui: &mut egui::Ui, title: &str, sub: &str, add: impl FnOnce(&mut egui::Ui)) {
    crate::palette::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(crate::palette::bold_text(title, 16.0).color(crate::palette::strong()));
        if !sub.is_empty() {
            ui.label(RichText::new(sub).weak().small());
        }
        ui.add_space(6.0);
        add(ui);
    });
    ui.add_space(10.0);
}

/// Word-spread buckets: "1", "2", "3–4", "5–9"… up to `books`.
fn spread_buckets(spread: &[usize]) -> Vec<(String, usize, usize, usize)> {
    let edges = [1, 2, 3, 5, 10, 20, 50, 100, 200, 500, 1000];
    let n = spread.len();
    let mut out = Vec::new();
    for (i, &lo) in edges.iter().enumerate() {
        if lo > n {
            break;
        }
        let hi = edges.get(i + 1).map_or(n, |&e| (e - 1).min(n));
        let count = spread[lo - 1..hi].iter().sum();
        let label = if lo == hi {
            lo.to_string()
        } else {
            format!("{lo}–{hi}")
        };
        out.push((label, lo, hi, count));
    }
    out
}

/// Unknown words listed as found in the most books.
const WIDESPREAD: usize = 100;

/// A word to mark known (true) or unmark: (word as counted, as shown, on).
type Toggle = (String, String, bool);

/// Layout of the saved stats; others are ignored and counted again.
const FORMAT: u32 = 3;

/// The Library's vocabulary stats as last counted. Saved in
/// `library-stats.json` so the tab is filled at start, before they are
/// counted again.
#[derive(Serialize, Deserialize)]
pub(crate) struct Counted {
    format: u32,
    /// Traditional and Simplified spellings were one word.
    pub(crate) fold: bool,
    /// History ids of the books counted, in the order of `words.book_known`.
    pub(crate) ids: Vec<String>,
    /// What counted as known ("Anki + HSK 1–5"); `None`: nothing.
    pub(crate) source: Option<String>,
    pub(crate) words: plan::stats::LibStats,
    pub(crate) chars: plan::stats::CharStats,
}

impl Counted {
    fn path() -> PathBuf {
        hanstat::models::app_dir().join("library-stats.json")
    }

    fn load() -> Option<Counted> {
        let c: Counted = serde_json::from_slice(&std::fs::read(Self::path()).ok()?).ok()?;
        (c.format == FORMAT).then_some(c)
    }

    /// Write under a temporary name, then rename.
    fn save(&self) -> Result<(), String> {
        let path = Self::path();
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        serde_json::to_vec(self)
            .map_err(|e| e.to_string())
            .and_then(|b| std::fs::write(&tmp, b).map_err(|e| e.to_string()))
            .and_then(|_| std::fs::rename(&tmp, &path).map_err(|e| e.to_string()))
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("Cannot save {}: {e}", path.display())
            })
    }
}

pub(crate) struct Done {
    counted: Counted,
    lib: (LibKey, Arc<plan::Library>),
    saved: Result<(), String>,
}

/// The Library's vocabulary stats, kept up to date in the background
/// whatever the window shows. The last count is shown until a newer one is
/// ready.
#[derive(Default)]
pub(crate) struct StatsState {
    pub(crate) shown: Option<Counted>,
    /// The library and the known words (`Arc` address; 0: none) of the
    /// running job, or of the last count.
    inputs: Option<(LibKey, usize)>,
    job: Option<Receiver<Done>>,
    /// The library last built, for the next count to reuse.
    lib: Option<(LibKey, Arc<plan::Library>)>,
    error: Option<String>,
}

impl StatsState {
    /// The counts saved by the last run.
    pub(crate) fn load() -> StatsState {
        StatsState {
            shown: Counted::load(),
            ..Default::default()
        }
    }
}

fn known_source(k: &Knowledge) -> String {
    match (k.from_anki, k.listed.source.is_empty()) {
        (true, true) => "Anki".to_owned(),
        (true, false) => format!("Anki + {}", k.listed.source),
        (false, _) => k.listed.source.clone(),
    }
}

impl App {
    /// Count the Library's vocabulary again when its books, their
    /// word data or the known words have changed: one count at a time, and
    /// not while the known words are still loading (that would replace the
    /// saved count with one against nothing known) or books are being
    /// analysed one after another.
    pub(crate) fn refresh_stats(&mut self, ctx: &egui::Context) {
        if self.stats.job.is_some()
            || self.anki.rx.is_some()
            || self.anki.dirty
            || self.batch.is_some()
        {
            return;
        }
        let have = self.plan_have();
        let key = self.lib_key(have);
        let k = self.known.merged.clone();
        let knowledge = k.as_ref().map_or(0, |k| Arc::as_ptr(k) as usize);
        if self
            .stats
            .inputs
            .as_ref()
            .is_some_and(|(lk, kn)| *lk == key && *kn == knowledge)
        {
            return;
        }
        self.stats.inputs = Some((key.clone(), knowledge));
        if key.ids.is_empty() {
            self.stats.shown = None;
            let _ = std::fs::remove_file(Counted::path());
            return;
        }
        let cached = [
            self.stats.lib.as_ref().map(|(k, l)| (k, l)),
            self.plan.lib.as_ref().map(|p| (&p.key, &p.lib)),
        ]
        .into_iter()
        .flatten()
        .find(|(lk, _)| **lk == key)
        .map(|(_, l)| l.clone());
        let titles: HashMap<&str, &str> = self
            .history
            .entries
            .iter()
            .map(|e| (e.id.as_str(), e.title.as_str()))
            .collect();
        let files: Vec<(String, PathBuf)> = key
            .ids
            .iter()
            .map(|id| {
                (
                    titles.get(id.as_str()).unwrap_or(&"?").to_string(),
                    self.history.profile(id),
                )
            })
            .collect();
        let source = k.as_deref().map(known_source).filter(|s| !s.is_empty());
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let books: Vec<(String, Profile)> = files
                .into_iter()
                .filter_map(|(t, p)| Profile::load(&p).map(|p| (t, p)))
                .collect();
            let profiles: Vec<Profile> = books.iter().map(|(_, p)| p.clone()).collect();
            let chars = plan::stats::char_stats(&profiles, k.as_deref(), key.fold);
            drop(profiles);
            let lib = cached
                .unwrap_or_else(|| Arc::new(plan::Library::build(key.fold, key.skip_hapax, books)));
            let words = lib.stats(&lib.known(k.as_deref()), WIDESPREAD);
            let counted = Counted {
                format: FORMAT,
                fold: lib.fold,
                ids: lib.books.iter().map(|b| b.id.clone()).collect(),
                source,
                words,
                chars,
            };
            let saved = counted.save();
            let _ = tx.send(Done {
                counted,
                lib: (key, lib),
                saved,
            });
            ctx.request_repaint();
        });
        self.stats.job = Some(rx);
    }

    pub(crate) fn poll_stats(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.stats.job else {
            return;
        };
        match rx.try_recv() {
            Ok(d) => {
                // Not for the reading path, which needs where words occur
                // (this library is built without them).
                self.stats.lib = Some(d.lib);
                self.stats.error = d.saved.err();
                self.stats.shown = Some(d.counted);
                self.stats.job = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100))
            }
            Err(_) => self.stats.job = None,
        }
    }

    pub(crate) fn stats_view(&mut self, ui: &mut egui::Ui) {
        if self.history.entries.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("No books analysed yet. Open or drop an .epub or .txt file.")
                        .size(22.0)
                        .weak(),
                );
            });
            return;
        }
        let mut toggle = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                // Keep clear of the scroll bar, which floats over the content.
                ui.set_max_width(ui.available_width() - 10.0);
                self.library_totals(ui);
                ui.add_space(14.0);
                self.reading_section(ui);
                ui.add_space(14.0);
                toggle = self.vocabulary_stats(ui);
            });
        // As + and − in the Words tab: the word joins the marked words, and
        // leaves the list when the stats are next counted, a moment later.
        if let Some((w, shown, on)) = toggle
            && self.known.settings.set_marked(&w, on)
        {
            let msg = if on {
                format!("Marked {shown} as known")
            } else {
                format!("Unmarked {shown}")
            };
            self.toast = Some((msg, ui.input(|i| i.time) + 3.0));
            self.known_changed();
        }
    }

    /// Every book, from the History.
    fn library_totals(&self, ui: &mut egui::Ui) {
        let speeds = self.speeds();
        let (mut han, mut words, mut minutes) = (0u64, 0u64, 0.0f64);
        // Running words of the books with a known share, and the known ones among them.
        let (mut known_of, mut known) = (0.0f64, 0.0f64);
        for e in &self.history.entries {
            let s = &e.stats;
            han += s.han_total;
            words += s.words_total;
            minutes += reading_minutes(s, speeds);
            if let Some(k) = &s.known {
                known_of += s.words_total as f64;
                known += s.words_total as f64 * k.running_words / 100.0;
            }
        }
        let books = self.history.entries.len();
        let read_ids: std::collections::HashSet<&str> = self
            .known
            .settings
            .read
            .iter()
            .map(|b| b.id.as_str())
            .collect();
        let read = self
            .history
            .entries
            .iter()
            .filter(|e| read_ids.contains(e.id.as_str()))
            .count();

        ui.label(RichText::new("Whole library").size(18.0).strong());
        ui.add_space(4.0);
        let mut tiles = vec![
            Tile::new(
                "Books",
                RichText::new(fmt_int(books as u64)),
                if read > 0 {
                    format!("{} marked read", fmt_int(read as u64))
                } else {
                    "none marked read".to_owned()
                },
            ),
            Tile::new(
                "Characters",
                RichText::new(fmt_int(han)),
                format!("{} per book", fmt_int(han / books.max(1) as u64)),
            )
            .hover("Every Script=Han character of every book's included sections, counted as on the Overview."),
            Tile::new(
                "Words",
                RichText::new(fmt_int(words)),
                "running words, as segmented".to_owned(),
            ),
            Tile::new(
                "Reading time",
                RichText::new(fmt_time(minutes)),
                format!("{:.0} days of reading 1 h a day", minutes / 60.0),
            )
            .hover("At your speed measured in the reader (unless set by hand in Settings, ⚙)"),
        ];
        if known_of > 0.0 {
            let p = known * 100.0 / known_of;
            let d = Difficulty::from_percent(p);
            tiles.push(
                Tile::new(
                    "Known",
                    RichText::new(format!("{p:.1}%")).color(difficulty_color(d)),
                    d.label().to_owned(),
                )
                .hover("Running words known at each book's last analysis, weighted by length; names left out"),
            );
        }
        if let Some(e) = self.history.entries.iter().max_by_key(|e| length(&e.stats)) {
            tiles.push(
                Tile::new(
                    "Longest book",
                    RichText::new(&e.title),
                    format!("{} characters", fmt_int(length(&e.stats))),
                )
                .hover(&e.title),
            );
        }
        tile_grid(ui, tiles);
    }

    /// The books against what is known now. Returns a word to mark known or
    /// unmark.
    fn vocabulary_stats(&self, ui: &mut egui::Ui) -> Option<Toggle> {
        let entries: Vec<&history::Entry> = self.history.entries.iter().collect();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Vocabulary").size(18.0).strong());
            if self.stats.job.is_some() {
                ui.spinner().on_hover_text(
                    "Counting again; the figures shown are from the last count until it is done",
                );
            }
        });
        if let Some(e) = &self.stats.error {
            ui.colored_label(crate::palette::warn(), format!("⚠ {e}"));
        }
        // The last count, until a newer one is ready.
        let res = self.stats.shown.as_ref();
        let source = res.and_then(|r| r.source.as_ref());
        let profiled = res.map_or(0, |r| r.ids.len());
        if res.is_some() && profiled < entries.len() {
            ui.label(
                RichText::new(format!(
                    "{} of the {} books have no word data yet and are left out of the vocabulary below; the Reading path tab can analyse them again.",
                    entries.len() - profiled,
                    entries.len()
                ))
                .weak(),
            );
        }
        ui.add_space(4.0);

        // Known now, by History id, for the books with word data.
        let known_now: std::collections::HashMap<&str, f64> = match (res, source.is_some()) {
            (Some(r), true) => r
                .ids
                .iter()
                .zip(&r.words.book_known)
                .map(|(id, &p)| (id.as_str(), p))
                .collect(),
            _ => Default::default(),
        };

        if let Some(r) = res {
            let w = &r.words;
            let c = &r.chars;
            let n = r.ids.len();
            let only_one = fmt_int(w.spread.first().copied().unwrap_or(0) as u64);
            let in_all = fmt_int(w.spread.last().copied().unwrap_or(0) as u64);
            let sub = if n >= 3 {
                format!("{in_all} in every book")
            } else {
                format!("{only_one} in one book only")
            };
            let mut tiles = vec![
                Tile::new("Distinct words", RichText::new(fmt_int(w.vocab as u64)), sub).hover(
                    &format!(
                        "Every word used in these books, counted once{}.\n{only_one} are in one book only, {in_all} in all {n}.",
                        if r.fold {
                            "; Traditional and Simplified spellings are one word, as for your known words"
                        } else {
                            ""
                        }
                    ),
                ),
            ];
            tiles.push(
                Tile::new(
                    "Distinct characters",
                    RichText::new(fmt_int(c.unique as u64)),
                    format!("{} in all", fmt_int(c.total)),
                )
                .hover(if r.fold {
                    "Characters of the books' words; Traditional and Simplified forms are one character, as for your known words"
                } else {
                    "Characters of the books' words, as written"
                }),
            );
            tiles.push(
                Tile::new(
                    "Words for 98%",
                    RichText::new(fmt_int(w.cover_at[3] as u64)),
                    format!("{} for 95%", fmt_int(w.cover_at[2] as u64)),
                )
                .hover("The most frequent words of the whole library that make up 98% (95%) of its running words"),
            );
            if let Some(src) = &source {
                let p = w.known_running;
                let d = Difficulty::from_percent(p);
                tiles.push(
                    Tile::new(
                        "Running words known",
                        RichText::new(format!("{p:.1}%")).color(difficulty_color(d)),
                        format!("{}, across every book", d.label()),
                    )
                    .hover(&format!(
                        "From {src}, now; names are left out, as on the Overview"
                    )),
                );
                tiles.push(
                    Tile::new(
                        "Words known",
                        RichText::new(format!("{:.1}%", pct(w.known_words, w.vocab))),
                        format!(
                            "{} of {} distinct words",
                            fmt_int(w.known_words as u64),
                            fmt_int(w.vocab as u64)
                        ),
                    )
                    .hover(&format!("From {src}, now")),
                );
                tiles.push(
                    Tile::new(
                        "Characters known",
                        RichText::new(format!("{:.1}%", pct(c.known, c.unique))),
                        format!(
                            "{} of {}",
                            fmt_int(c.known as u64),
                            fmt_int(c.unique as u64)
                        ),
                    )
                    .hover("A character is known when a known word contains it"),
                );
            }
            tile_grid(ui, tiles);
            ui.add_space(6.0);
            if source.is_some() && n > 0 {
                let mut counts = [0usize; 5];
                for (id, &p) in r.ids.iter().zip(&w.book_known) {
                    counts[Difficulty::from_percent(p - self.style_offset(id)) as usize] += 1;
                }
                let parts: Vec<(usize, Color32, String)> = [
                    Difficulty::Easy,
                    Difficulty::Moderate,
                    Difficulty::Challenging,
                    Difficulty::Hard,
                    Difficulty::VeryHard,
                ]
                .into_iter()
                .map(|d| (counts[d as usize], difficulty_fill(d), d.label().to_owned()))
                .collect();
                ui.label(RichText::new("Difficulty now").weak());
                charts::stacked_bar(ui, &parts);
                ui.add_space(8.0);
            }
        } else if self.stats.job.is_some() || self.anki.rx.is_some() || self.batch.is_some() {
            ui.label(RichText::new("Counting the vocabulary…").weak());
        }

        let wide = ui.available_width() > 980.0;
        let charts_left = |ui: &mut egui::Ui| {
            self.length_chart(ui, &entries, &known_now);
            if let Some(r) = res {
                self.coverage_chart(ui, r, source.is_some());
            }
        };
        let charts_right = |ui: &mut egui::Ui| {
            if !known_now.is_empty() {
                self.scatter_chart(ui, &entries, &known_now);
            }
            if let Some(r) = res {
                spread_chart(ui, r);
            }
        };
        if wide {
            ui.columns(2, |cols| {
                charts_left(&mut cols[0]);
                charts_right(&mut cols[1]);
            });
        } else {
            charts_left(ui);
            charts_right(ui);
        }
        self.read_section(ui, &entries);
        let mut toggle = None;
        if let Some(r) = res
            && source.is_some()
        {
            let marked = |w: &str| self.known.settings.is_marked(w);
            toggle = widespread_table(ui, r, &marked);
        }
        toggle
    }

    fn length_chart(
        &self,
        ui: &mut egui::Ui,
        entries: &[&history::Entry],
        known_now: &std::collections::HashMap<&str, f64>,
    ) {
        let mut rows: Vec<&&history::Entry> = entries.iter().collect();
        rows.sort_by_key(|e| std::cmp::Reverse(length(&e.stats)));
        let bars: Vec<Bar> = rows
            .iter()
            .map(|e| {
                let len = length(&e.stats);
                let now = known_now.get(e.id.as_str()).copied();
                let mut hover = format!("{}\n{} characters", e.title, fmt_int(len));
                let color = match now {
                    Some(p) => {
                        let d = Difficulty::from_percent(p - self.style_offset(&e.id));
                        hover.push_str(&format!(
                            "\n{p:.1}% of running words known now · {}",
                            d.label()
                        ));
                        difficulty_fill(d)
                    }
                    None => crate::palette::accent(),
                };
                Bar {
                    value: len as f64,
                    color,
                    hover,
                }
            })
            .collect();
        let sub = if known_now.is_empty() {
            "Longest first".to_owned()
        } else {
            "Longest first, coloured by how hard each is now".to_owned()
        };
        card(ui, "Book lengths (characters)", &sub, |ui| {
            charts::columns(ui, &bars, None, CHART_HEIGHT, &charts::compact);
        });
    }

    fn scatter_chart(
        &self,
        ui: &mut egui::Ui,
        entries: &[&history::Entry],
        known_now: &std::collections::HashMap<&str, f64>,
    ) {
        let points: Vec<Point> = entries
            .iter()
            .filter_map(|e| {
                let p = *known_now.get(e.id.as_str())?;
                let len = length(&e.stats);
                let d = Difficulty::from_percent(p - self.style_offset(&e.id));
                Some(Point {
                    x: len.max(1) as f64,
                    y: p,
                    color: difficulty_fill(d),
                    hover: format!(
                        "{}\n{} characters · {p:.1}% known · {}",
                        e.title,
                        fmt_int(len),
                        d.label()
                    ),
                })
            })
            .collect();
        let lowest = points.iter().map(|p| p.y).fold(100.0, f64::min);
        let y_lo = ((lowest - 2.0).min(78.0) / 5.0).floor().max(0.0) * 5.0;
        let mut bands = Vec::new();
        let mut hi = 100.0;
        for (lo, d) in Difficulty::BANDS {
            bands.push(Band {
                lo,
                hi,
                color: difficulty_fill(d),
                label: d.label(),
            });
            hi = lo;
        }
        bands.push(Band {
            lo: 0.0,
            hi,
            color: difficulty_fill(Difficulty::VeryHard),
            label: Difficulty::VeryHard.label(),
        });
        card(
            ui,
            "Length and difficulty",
            "Running words known now against length in characters (log scale)",
            |ui| charts::scatter(ui, &points, &bands, (y_lo, 100.0), CHART_HEIGHT),
        );
    }

    fn coverage_chart(&self, ui: &mut egui::Ui, r: &Counted, known: bool) {
        let w = &r.words;
        let marks: Vec<(f64, usize)> = plan::stats::COVER_AT
            .iter()
            .copied()
            .zip(w.cover_at)
            .filter(|(p, _)| *p >= 90.0)
            .collect();
        let level = known.then(|| {
            (
                w.known_running,
                format!("You know {:.1}% of the running words", w.known_running),
            )
        });
        card(
            ui,
            "Vocabulary coverage",
            "How much of the whole library its most frequent words make up (log scale)",
            |ui| {
                charts::coverage_curve(
                    ui,
                    &w.coverage,
                    &marks,
                    level,
                    crate::palette::accent(),
                    "words",
                    CHART_HEIGHT,
                )
            },
        );
    }
}

fn spread_chart(ui: &mut egui::Ui, r: &Counted) {
    let w = &r.words;
    let buckets = spread_buckets(&w.spread);
    let n = w.spread.len();
    let bars: Vec<Bar> = buckets
        .iter()
        .map(|(label, lo, hi, count)| Bar {
            value: *count as f64,
            color: crate::palette::accent(),
            hover: format!(
                "{} words ({:.1}%) are in {} of the {n} books",
                fmt_int(*count as u64),
                pct(*count, w.vocab),
                if lo == hi {
                    label.clone()
                } else {
                    format!("{lo} to {hi}")
                },
            ),
        })
        .collect();
    let labels: Vec<String> = buckets.into_iter().map(|b| b.0).collect();
    card(
        ui,
        "How widely words are shared",
        "Distinct words by the number of books they appear in",
        |ui| charts::columns(ui, &bars, Some(&labels), CHART_HEIGHT, &charts::compact),
    );
}

/// The unknown words found in the most books, each with + to mark it known
/// (− unmarks one marked since the last count).
fn widespread_table(
    ui: &mut egui::Ui,
    r: &Counted,
    is_marked: &dyn Fn(&str) -> bool,
) -> Option<Toggle> {
    let w = &r.words;
    if w.widespread.is_empty() {
        return None;
    }
    let n = r.ids.len();
    let mut toggle = None;
    card(
        ui,
        "Unknown words in the most books",
        "Not known now and not names: the words worth studying for the library as a whole. + marks a word known, as in the Words tab.",
        |ui| {
            egui::ScrollArea::vertical()
                .id_salt("stats_widespread")
                .max_height(320.0)
                // Inside the page's own scroll area it would otherwise
                // shrink to the space left on screen.
                .min_scrolled_height(320.0)
                .show(ui, |ui| {
                    // Side by side in as many groups as fit, so a wide
                    // window shows more of the list.
                    const GROUP: f32 = 380.0;
                    let groups = ((ui.available_width() / GROUP).floor() as usize).max(1);
                    let per = w.widespread.len().div_ceil(groups);
                    ui.horizontal_top(|ui| {
                        for (g, chunk) in w.widespread.chunks(per).enumerate() {
                            ui.allocate_ui(egui::vec2(GROUP - 16.0, 0.0), |ui| {
                                egui::Grid::new(("stats_widespread_grid", g))
                                    .striped(true)
                                    .spacing([16.0, 4.0])
                                    // The default minimum (a button's width) spreads
                                    // out the rank and + columns.
                                    .min_col_width(0.0)
                                    .show(ui, |ui| {
                                        ui.strong("#");
                                        ui.label("");
                                        ui.strong("Word");
                                        ui.strong("Books");
                                        ui.strong("Occurrences");
                                        ui.end_row();
                                        for (i, x) in chunk.iter().enumerate() {
                                            let rank = g * per + i + 1;
                                            ui.label(RichText::new(rank.to_string()).weak());
                                            let marked = is_marked(&x.word);
                                            if marked {
                                                if ui
                                                    .small_button("−")
                                                    .on_hover_text("You marked this word known. Click to unmark it.")
                                                    .clicked()
                                                {
                                                    toggle = Some((x.word.clone(), x.shown.clone(), false));
                                                }
                                            } else if ui
                                                .small_button("+")
                                                .on_hover_text(
                                                    "Mark as known (added to your known words, kept between runs)",
                                                )
                                                .clicked()
                                            {
                                                toggle = Some((x.word.clone(), x.shown.clone(), true));
                                            }
                                            let mut text = RichText::new(&x.shown).size(16.0);
                                            if marked {
                                                text = text.weak().strikethrough();
                                            }
                                            let l = ui.label(text);
                                            if x.shown != x.word {
                                                l.on_hover_text(format!(
                                                    "Counted as {}, which merges its spellings and forms",
                                                    x.word
                                                ));
                                            }
                                            ui.label(format!("{} of {n}", x.books));
                                            ui.label(fmt_int(x.count));
                                            ui.end_row();
                                        }
                                    });
                            });
                            ui.add_space(16.0);
                        }
                    });
                });
        },
    );
    toggle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_cover_every_book_count() {
        let spread = vec![10, 5, 3, 2, 1, 1, 0];
        let b = spread_buckets(&spread);
        let labels: Vec<&str> = b.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(labels, ["1", "2", "3–4", "5–7"]);
        assert_eq!(b.iter().map(|x| x.3).sum::<usize>(), 22);
        assert_eq!(b[2].3, 5);
    }

    /// A saved count reads back, including an empty library's (no NaN).
    #[test]
    fn counted_round_trips() {
        let lib = plan::Library::build(false, false, Vec::new());
        let c = Counted {
            format: FORMAT,
            fold: false,
            ids: Vec::new(),
            source: Some("Anki".into()),
            words: lib.stats(&lib.known(None), WIDESPREAD),
            chars: plan::stats::char_stats(&[], None, false),
        };
        let back: Counted = serde_json::from_slice(&serde_json::to_vec(&c).unwrap()).unwrap();
        assert_eq!(back.source.as_deref(), Some("Anki"));
        assert_eq!(back.words.known_running, 100.0);
    }
}
