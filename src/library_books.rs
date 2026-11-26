//! Library › Books: the table of every book analysed, with the columns the
//! user chose ([`crate::library_columns`]): sorted and filtered by any of
//! them, the columns that don't fit on further pages, the Columns window,
//! and the user's own columns edited in place.

use crate::icons::{
    self, ARROW_CLOCKWISE, ARROW_U_UP_LEFT, BOOK_OPEN, CALENDAR, CARET_DOWN, CARET_LEFT,
    CARET_RIGHT, CARET_UP, CHECK, COLUMNS, DOTS_SIX_VERTICAL, GEAR, PENCIL, STAR, TRASH, X,
};
use crate::library_columns::{
    self as lc, Columns, Deleted, FieldType, Filter, FilterKind, HistCol, Key, STATS, Val, Value,
};
use crate::reading_stats::{self as rs, LeftAt};
use crate::{
    App, COVER_ROW, COVER_SIZE, READ_COL, UNDO_SECS, book_matches, chapter_lengths,
    difficulty_color, fmt_int, load_cover, palette, script_badge, sort_header,
};
use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use hanstat::anki::Difficulty;
use hanstat::history;
use hanstat::script::Variant;
use hanstat::search;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

/// The ✔ Read column, and the ✖ column's least width.
const MARK_COL: f32 = 64.0;
const REMOVE_COL: f32 = 36.0;
/// The Book column's width when titles are long and columns many.
const BOOK_MIN: f32 = 140.0;
const BOOK_FLOOR_MAX: f32 = 280.0;
/// Room in a header for the sort arrow and the filter button.
const HEADER_EXTRA: f32 = 40.0;

/// What the Books tab keeps between frames.
pub(crate) struct ColumnsState {
    pub(crate) cols: Columns,
    /// A user column just deleted, while it can be put back, and until when.
    deleted: Option<(Deleted, f64)>,
    /// The first column of the page shown: after a resize, the page that
    /// holds it is shown.
    page_at: Option<HistCol>,
    pub(crate) window_open: bool,
    /// The cell being typed in: book id, field id, the text, whether it has
    /// had the keyboard yet.
    edit: Option<(String, u32, String, bool)>,
    /// A user column being made: its name and type.
    new_field: Option<(String, FieldType)>,
    /// A user column being renamed.
    rename: Option<(u32, String)>,
    /// Saving failed: shown as an error, once.
    pub(crate) error: Option<String>,
    /// The reading-stats columns' figures, by book, for a stats revision.
    reading: Option<(u64, Arc<HashMap<String, ReadInfo>>)>,
}

impl Default for ColumnsState {
    fn default() -> Self {
        ColumnsState {
            cols: if cfg!(test) {
                Columns::default()
            } else {
                Columns::load()
            },
            deleted: None,
            page_at: None,
            window_open: false,
            edit: None,
            new_field: None,
            rename: None,
            error: None,
            reading: None,
        }
    }
}

impl ColumnsState {
    pub(crate) fn save(&mut self) {
        if cfg!(test) {
            return;
        }
        if let Err(e) = self.cols.save() {
            self.error = Some(e);
        }
    }

    /// Book `old` is now `new`: its values follow.
    pub(crate) fn rekey(&mut self, old: &str, new: &str) {
        if self.cols.rekey(old, new) {
            self.save();
        }
    }

    /// A book is gone for good: its values go too.
    pub(crate) fn forget(&mut self, id: &str) {
        if self.cols.forget(id) {
            self.save();
        }
    }

    /// The deleted column can no longer be put back: saved without it.
    pub(crate) fn finish_delete(&mut self) {
        if self.deleted.take().is_some() {
            self.save();
        }
    }

    pub(crate) fn tick(&mut self, now: f64, ctx: &egui::Context) {
        if let Some((_, until)) = &self.deleted {
            if now >= *until {
                self.finish_delete();
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(until - now));
            }
        }
    }

    /// The status bar's "Deleted … · Undo".
    pub(crate) fn undo_ui(&mut self, ui: &mut egui::Ui) {
        let Some((d, _)) = &self.deleted else { return };
        let undo = ui
            .button(icons::rich(ui, format!("{ARROW_U_UP_LEFT} Undo")))
            .on_hover_text("Put the column back, with its values")
            .clicked();
        let n = d.values.len();
        ui.label(format!(
            "Deleted the column “{}” ({} {}).",
            d.field.name,
            n,
            if n == 1 { "value" } else { "values" }
        ));
        if undo && let Some((d, _)) = self.deleted.take() {
            self.cols.restore(d);
            self.save();
        }
    }

    /// The reading-stats figures by book, made again when the stats change.
    fn reading(&mut self) -> Arc<HashMap<String, ReadInfo>> {
        let rev = rs::revision();
        if let Some((r, m)) = &self.reading
            && *r == rev
        {
            return m.clone();
        }
        let m: HashMap<String, ReadInfo> = rs::with(|s| {
            s.books
                .iter()
                .map(|(id, b)| {
                    let t = b.totals();
                    (
                        id.clone(),
                        ReadInfo {
                            secs: t.read_secs,
                            speed: b.speed(),
                            first: b.first_read,
                            last_day: b.days.keys().next_back().copied(),
                            finished: b.finished,
                        },
                    )
                })
                .collect()
        });
        let m = Arc::new(m);
        self.reading = Some((rev, m.clone()));
        m
    }
}

/// A book's reading in the reader.
#[derive(Clone, Copy, Debug, Default)]
struct ReadInfo {
    secs: f64,
    speed: Option<f64>,
    first: Option<u64>,
    last_day: Option<i32>,
    finished: Option<u64>,
}

/// What the cells are drawn from, made once per frame.
struct Data<'a> {
    entries: &'a [history::Entry],
    cols: &'a Columns,
    /// Characters per minute, for the reading time.
    cpm: f64,
    style_scores: HashMap<&'a str, f32>,
    style_scale: &'a crate::StyleScale,
    style_skipped: &'a HashMap<String, String>,
    style_off: HashMap<&'a str, f64>,
    read_at: HashMap<String, u64>,
    reading: Arc<HashMap<String, ReadInfo>>,
    /// The series of the books in one with another book: its name and the
    /// volume, and whether it came from the metadata.
    series: HashMap<&'a str, (String, Vec<u32>, bool)>,
    now: u64,
}

impl Data<'_> {
    fn place(&self, id: &str) -> Option<LeftAt> {
        rs::left_at(id)
    }

    /// The book's progress, 0–100, if known: 100 once finished in the
    /// reader or marked read.
    fn progress(&self, i: usize) -> Option<f64> {
        let e = &self.entries[i];
        if self.read_at.contains_key(&e.id)
            || self
                .reading
                .get(&e.id)
                .is_some_and(|r| r.finished.is_some())
        {
            return Some(100.0);
        }
        let at = self.place(&e.id)?;
        let ch = chapter_lengths::get(&e.id)?;
        Some(chapter_lengths::through(&ch, at) as f64 * 100.0)
    }

    /// The row's value in a column, in the column's unit.
    fn val(&self, c: HistCol, i: usize) -> Val {
        let e = &self.entries[i];
        let s = &e.stats;
        let r = self.reading.get(&e.id);
        let num = |x: u64| Val::Num(x as f64);
        let opt = |x: Option<f64>| x.map_or(Val::None, Val::Num);
        match c {
            HistCol::Title => Val::Text(e.title.clone()),
            HistCol::Language => Val::Choice(s.variant.clone(), 0.0),
            HistCol::Chars => num(s.han_total),
            HistCol::Words => num(s.words_total),
            HistCol::Unique => num(s.words_unique),
            HistCol::Known => opt(s.known.as_ref().map(|k| k.running_words)),
            HistCol::Difficulty => match &s.known {
                Some(k) => {
                    let counted =
                        k.running_words - self.style_off.get(e.id.as_str()).copied().unwrap_or(0.0);
                    // Harder first when descending.
                    Val::Choice(
                        Difficulty::from_percent(counted).label().to_owned(),
                        100.0 - counted,
                    )
                }
                None => Val::None,
            },
            HistCol::Style => opt(self
                .style_scores
                .get(e.id.as_str())
                .map(|&x| self.style_scale.share(x) as f64)),
            HistCol::Time => Val::Num(s.han_total as f64 / self.cpm / 60.0),
            HistCol::Analysed => Val::Time(e.analysed as f64),
            HistCol::Read => self
                .read_at
                .get(&e.id)
                .map_or(Val::None, |&t| Val::Time(t as f64)),
            HistCol::Progress => opt(self.progress(i)),
            HistCol::UniqueChars => num(s.han_unique),
            HistCol::Words98 => {
                if s.words_for_98 > 0 {
                    num(s.words_for_98)
                } else {
                    Val::None
                }
            }
            HistCol::KnownUnique => opt(s.known.as_ref().map(|k| k.unique_words)),
            HistCol::Unknown => opt(s.known.as_ref().map(|k| k.unknown_words as f64)),
            HistCol::KnownChars => opt(s.known.as_ref().map(|k| k.unique_chars)),
            HistCol::Segmenter => Val::Choice(s.segmenter.clone(), 0.0),
            HistCol::Format => Val::Choice(s.format.clone(), 0.0),
            HistCol::Sections => num(s.sections_included as u64),
            HistCol::TimeRead => match r {
                Some(r) if r.secs > 0.0 => Val::Num(r.secs / 3600.0),
                _ => Val::None,
            },
            HistCol::Speed => opt(r.and_then(|r| r.speed)),
            HistCol::FirstRead => r
                .and_then(|r| r.first)
                .map_or(Val::None, |t| Val::Time(t as f64)),
            HistCol::LastRead => r.and_then(|r| r.last_day).map_or(Val::None, Val::Day),
            HistCol::Finished => r
                .and_then(|r| r.finished)
                .map_or(Val::None, |t| Val::Time(t as f64)),
            HistCol::Series => match self.series.get(e.id.as_str()) {
                Some((name, vol, _)) => {
                    Val::Text(format!("{name} · {}", hanstat::series::volume_label(vol)))
                }
                None => Val::None,
            },
            HistCol::Field(id) => {
                let ty = self.cols.field(id).map(|f| f.ty);
                match (ty, self.cols.value(&e.id, id)) {
                    (Some(FieldType::YesNo), v) => {
                        let yes = matches!(v, Some(Value::Yes(true)));
                        Val::Choice(if yes { "Yes" } else { "No" }.to_owned(), yes as u8 as f64)
                    }
                    (_, Some(Value::Text(t))) if !t.is_empty() => Val::Text(t.clone()),
                    (_, Some(Value::Number(x))) => Val::Num(*x),
                    (_, Some(Value::Stars(n))) => Val::Num(*n as f64),
                    (_, Some(Value::Date(d))) => Val::Day(*d),
                    _ => Val::None,
                }
            }
        }
    }

    /// What rows sort by in a column.
    fn key(&self, c: HistCol, i: usize) -> Key {
        if c == HistCol::Series {
            // By series, then volume: the volume as fixed-width numbers.
            return match self.series.get(self.entries[i].id.as_str()) {
                Some((name, vol, _)) => {
                    let mut k = name.to_lowercase();
                    k.push('\0');
                    for n in vol {
                        k.push_str(&format!("{n:010}"));
                    }
                    Key::Text(k)
                }
                None => Key::None,
            };
        }
        self.val(c, i).key()
    }

    /// The labels a Choice column's values take, in sorting order.
    fn choices(&self, c: HistCol) -> Vec<String> {
        match c {
            HistCol::Difficulty => Difficulty::BANDS
                .iter()
                .map(|(_, d)| d.label().to_owned())
                .chain([Difficulty::VeryHard.label().to_owned()])
                .collect(),
            HistCol::Field(_) => vec!["Yes".to_owned(), "No".to_owned()],
            _ => {
                let mut seen: BTreeSet<String> = BTreeSet::new();
                for i in 0..self.entries.len() {
                    if let Val::Choice(l, _) = self.val(c, i) {
                        seen.insert(l);
                    }
                }
                seen.into_iter().collect()
            }
        }
    }
}

/// What the table asks for, done once it is drawn.
enum Act {
    Open(PathBuf),
    Read(PathBuf, String, bool),
    Remove(String),
    MarkRead(String),
    Sort((HistCol, bool)),
    Filter(HistCol, Filter),
    Value(String, u32, Option<Value>),
    Edit(String, u32, String),
    Page(HistCol),
}

/// Today's calendar day (local midnight to midnight), days since 1970-01-01.
fn today() -> i32 {
    rs::local_day(rs::now(), 0)
}

fn day_of(t: f64) -> i32 {
    rs::local_day(t, 0)
}

fn hm(hours: f64) -> String {
    let mins = hours * 60.0;
    if mins > 0.0 && mins < 1.0 {
        return "<1m".to_owned();
    }
    format!("{}h {:02}m", (mins / 60.0) as u64, (mins % 60.0) as u64)
}

fn dash() -> RichText {
    RichText::new("–").weak()
}

/// A small funnel: the filter button of a column header, in the accent
/// colour while a filter is set.
fn filter_button(ui: &mut egui::Ui, active: bool, name: &str) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Filter {name}")));
    let colour = if active {
        palette::accent()
    } else if r.hovered() {
        ui.visuals().strong_text_color()
    } else {
        palette::weak()
    };
    let c = rect.center();
    let pts = vec![
        egui::pos2(c.x - 5.0, c.y - 4.5),
        egui::pos2(c.x + 5.0, c.y - 4.5),
        egui::pos2(c.x + 1.2, c.y + 0.5),
        egui::pos2(c.x + 1.2, c.y + 4.5),
        egui::pos2(c.x - 1.2, c.y + 5.5),
        egui::pos2(c.x - 1.2, c.y + 0.5),
    ];
    let shape = if active {
        egui::Shape::convex_polygon(pts, colour, egui::Stroke::NONE)
    } else {
        egui::Shape::closed_line(pts, egui::Stroke::new(1.2, colour))
    };
    ui.painter().add(shape);
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A day number as a date picker's date.
fn to_date(day: i32) -> jiff::civil::Date {
    let (y, m, d) = rs::civil(day as i64);
    jiff::civil::Date::new(y as i16, m as i8, d as i8).unwrap_or_default()
}

fn from_date(d: jiff::civil::Date) -> i32 {
    rs::days_from_civil(d.year() as i64, d.month() as i64, d.day() as i64) as i32
}

/// A date picker for an optional day: "Set" when there is none, ✖ to clear.
fn day_picker(ui: &mut egui::Ui, day: &mut Option<i32>, salt: &str) -> bool {
    day_picker_from(ui, day, salt, today(), "Today; change it with the calendar")
}

/// [`day_picker`] whose "Set" picks `start` (`tip` says which day that is).
pub(crate) fn day_picker_from(
    ui: &mut egui::Ui,
    day: &mut Option<i32>,
    salt: &str,
    start: i32,
    tip: &str,
) -> bool {
    let mut changed = false;
    match *day {
        Some(d) => {
            let mut date = to_date(d);
            ui.add(egui_extras::DatePickerButton::new(&mut date).id_salt(salt));
            let nd = from_date(date);
            if nd != d {
                *day = Some(nd);
                changed = true;
            }
            if ui
                .small_button(icons::glyph(ui.ctx(), X, false))
                .on_hover_text("No date")
                .clicked()
            {
                *day = None;
                changed = true;
            }
        }
        None => {
            if ui
                .button(icons::rich(ui, format!("{CALENDAR} Set")))
                .on_hover_text(tip)
                .clicked()
            {
                *day = Some(start);
                changed = true;
            }
        }
    }
    changed
}

/// A column's unit, after the numbers in its filter.
fn unit(c: HistCol) -> &'static str {
    match c {
        HistCol::Known
        | HistCol::KnownUnique
        | HistCol::KnownChars
        | HistCol::Style
        | HistCol::Progress => "%",
        HistCol::Time | HistCol::TimeRead => "h",
        HistCol::Speed => "cpm",
        _ => "",
    }
}

/// What a filter does, as its chip says it.
fn describe(c: HistCol, name: &str, f: &Filter) -> String {
    let u = unit(c);
    let n = |x: f64| {
        let t = format!("{x:.1}");
        t.strip_suffix(".0").map(str::to_owned).unwrap_or(t)
    };
    let mut parts: Vec<String> = Vec::new();
    match (f.min.map(n), f.max.map(n)) {
        (Some(a), Some(b)) => parts.push(format!("{a}–{b}{u}")),
        (Some(a), None) => parts.push(format!("≥ {a}{u}")),
        (None, Some(b)) => parts.push(format!("≤ {b}{u}")),
        _ => {}
    }
    if !f.picked.is_empty() {
        parts.push(f.picked.iter().cloned().collect::<Vec<_>>().join(", "));
    }
    if !f.text.trim().is_empty() {
        parts.push(format!("“{}”", f.text.trim()));
    }
    if let Some(n) = f.within {
        parts.push(format!("last {n} d"));
    }
    if let Some(n) = f.older {
        parts.push(format!("> {n} d ago"));
    }
    match (f.from, f.to) {
        (Some(a), Some(b)) => parts.push(format!("{} – {}", lc::date_label(a), lc::date_label(b))),
        (Some(a), None) => parts.push(format!("from {}", lc::date_label(a))),
        (None, Some(b)) => parts.push(format!("to {}", lc::date_label(b))),
        _ => {}
    }
    if c == HistCol::Read && !f.has_bounds() {
        return match (f.values, f.blanks) {
            (false, true) => "Not read".to_owned(),
            (true, false) => "Read".to_owned(),
            _ => "Read: none".to_owned(),
        };
    }
    if !f.values {
        parts.push("only –".to_owned());
    } else if f.blanks && f.has_bounds() {
        parts.push("or –".to_owned());
    } else if !f.blanks && !f.has_bounds() {
        parts.push("has a value".to_owned());
    }
    format!("{name}: {}", parts.join(" · "))
}

/// The filter editor of column `c`: changes `f`, true if it changed.
fn filter_editor(
    ui: &mut egui::Ui,
    c: HistCol,
    kind: FilterKind,
    f: &mut Filter,
    choices: &[String],
    span: (f64, f64),
) -> bool {
    let before = f.clone();
    let had = f.has_bounds();
    ui.set_min_width(240.0);
    let u = unit(c);
    match kind {
        FilterKind::Range => {
            let step = ((span.1 - span.0).abs() / 200.0).max(0.1);
            let bound = |ui: &mut egui::Ui, label: &str, v: &mut Option<f64>, start: f64| {
                ui.horizontal(|ui| {
                    let mut on = v.is_some();
                    ui.checkbox(&mut on, label);
                    if on {
                        let mut x = v.unwrap_or(start);
                        ui.add(egui::DragValue::new(&mut x).speed(step).suffix(u));
                        *v = Some(x);
                    } else {
                        *v = None;
                    }
                });
            };
            bound(ui, "At least", &mut f.min, span.0);
            bound(ui, "At most", &mut f.max, span.1);
        }
        FilterKind::Choice => {
            ui.label(RichText::new("Show only (none ticked: all)").weak());
            for l in choices {
                let mut on = f.picked.contains(l);
                if ui.checkbox(&mut on, l).changed() {
                    if on {
                        f.picked.insert(l.clone());
                    } else {
                        f.picked.remove(l);
                    }
                }
            }
        }
        FilterKind::Text => {
            ui.horizontal(|ui| {
                ui.label("Contains");
                let r = ui.add(egui::TextEdit::singleline(&mut f.text).desired_width(160.0));
                r.on_hover_text(crate::LIBRARY_SEARCH_HOVER);
            });
        }
        FilterKind::Age => {
            let days = |ui: &mut egui::Ui, label: &str, v: &mut Option<u32>, start: u32| {
                ui.horizontal(|ui| {
                    let mut on = v.is_some();
                    ui.checkbox(&mut on, label);
                    if on {
                        let mut x = v.unwrap_or(start);
                        ui.add(
                            egui::DragValue::new(&mut x)
                                .range(0..=36_500)
                                .suffix(" days"),
                        );
                        *v = Some(x);
                    } else {
                        *v = None;
                    }
                });
            };
            days(ui, "In the last", &mut f.within, 30);
            days(ui, "More than", &mut f.older, 365);
            ui.horizontal(|ui| {
                ui.label("From");
                day_picker(ui, &mut f.from, "filter-from");
            });
            ui.horizontal(|ui| {
                ui.label("To");
                day_picker(ui, &mut f.to, "filter-to");
            });
        }
    }
    ui.separator();
    let (with, without) = if c == HistCol::Read {
        ("Books marked read", "Books not marked read")
    } else {
        ("Books with a value", "Books with no value (–)")
    };
    ui.checkbox(&mut f.values, with);
    if ui.checkbox(&mut f.blanks, without).changed() {
        f.blanks_set = true;
    }
    if ui.button("Clear").clicked() {
        *f = Filter::default();
    }
    f.settle(had);
    *f != before
}

impl App {
    /// The Library header's "Columns…" button.
    pub(crate) fn columns_button(&mut self, ui: &mut egui::Ui) {
        if ui
            .selectable_label(
                self.columns.window_open,
                icons::rich(ui, format!("{COLUMNS} Columns…")),
            )
            .on_hover_text("Choose, order and add the Library's columns, including your own")
            .clicked()
        {
            self.columns.window_open = !self.columns.window_open;
        }
    }

    /// The Books tab below the Library's header.
    pub(crate) fn books_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.columns.window_open {
            self.columns_window(ctx);
        }
        let style_on = self.style_on();
        // Sorted by a column hidden or removed: back to the newest first.
        let (sc, _) = self.history_sort;
        if !self.columns.cols.is_shown(sc) {
            self.history_sort = (HistCol::Analysed, true);
        }
        let (col, desc) = self.history_sort;
        let style_scores: HashMap<&str, f32> = if style_on {
            self.style.store.current()
        } else {
            HashMap::new()
        };
        let style_scale = self.style_scale();
        let style_off: HashMap<&str, f64> = self
            .history
            .entries
            .iter()
            .map(|e| (e.id.as_str(), self.style_offset(&e.id)))
            .collect();
        let reading = self.columns.reading();
        let cols = &self.columns.cols;
        let entries = &self.history.entries;
        // Which columns need what: work is only done for those in use.
        let used = |c: HistCol| cols.is_shown(c) || cols.filter(c).is_some() || col == c;
        let mut series = HashMap::new();
        if used(HistCol::Series) {
            let metas: Vec<Option<hanstat::series::Meta>> = entries
                .iter()
                .map(|e| crate::book_series::meta(&e.id))
                .collect();
            let named: Vec<(hanstat::series::Volume, String)> = entries
                .iter()
                .zip(&metas)
                .map(|(e, m)| hanstat::series::named(&e.title, m.as_ref()))
                .collect();
            let vols: Vec<hanstat::series::Volume> = named.iter().map(|(v, _)| v.clone()).collect();
            let groups = hanstat::series::group(&vols);
            // A series is named by a book whose metadata names it, else by
            // its first book.
            let mut names: HashMap<u32, (String, bool)> = HashMap::new();
            for (i, g) in groups.iter().enumerate() {
                if let Some((gid, _)) = g {
                    let meta = metas[i].is_some();
                    let e = names
                        .entry(*gid)
                        .or_insert_with(|| (named[i].1.clone(), meta));
                    if meta && !e.1 {
                        *e = (named[i].1.clone(), true);
                    }
                }
            }
            for (i, g) in groups.into_iter().enumerate() {
                if let Some((gid, vol)) = g {
                    let (name, _) = &names[&gid];
                    series.insert(
                        entries[i].id.as_str(),
                        (name.clone(), vol, metas[i].is_some()),
                    );
                }
            }
        }
        let data = Data {
            entries,
            cols,
            cpm: self.speeds(),
            style_scores,
            style_scale: &style_scale,
            style_skipped: &self.style.skipped,
            style_off,
            read_at: self.read_dates(),
            reading,
            series,
            now: history::now(),
        };
        // Progress: chapter lengths worked out for the books read in the reader.
        if used(HistCol::Progress) {
            for e in entries.iter() {
                if crate::reader::opens(&e.stats.format)
                    && !data.read_at.contains_key(&e.id)
                    && rs::left_at(&e.id).is_some()
                    && chapter_lengths::get(&e.id).is_none()
                {
                    chapter_lengths::want(&e.id, &e.path, ctx);
                }
            }
        }

        // Rows: the search (title, file name, the user's Text columns) and
        // every filter.
        let query = search::Query::new(&self.library_filter);
        let today = today();
        let mut rows: Vec<usize> = (0..entries.len())
            .filter(|&i| {
                let e = &entries[i];
                book_matches(&query, &e.title, &e.path, cols.texts(&e.id))
            })
            .filter(|&i| {
                cols.filters
                    .iter()
                    .all(|(c, f)| f.passes(&data.val(*c, i), today, &day_of))
            })
            .collect();
        rows.sort_by_cached_key(|&i| data.key(col, i));
        if desc {
            rows.reverse();
        }

        let mut acts: Vec<Act> = Vec::new();
        // Pages: the shown columns between the pinned ones, as many as fit.
        let page_cols: Vec<HistCol> = cols
            .shown()
            .filter(|c| style_on || *c != HistCol::Style)
            .collect();
        let gap = ui.spacing().item_spacing.x;
        let font = egui::TextStyle::Body.resolve(ui.style());
        let text_w = |t: &str| {
            ui.painter()
                .layout_no_wrap(t.to_owned(), font.clone(), Color32::WHITE)
                .size()
                .x
        };
        let widths: Vec<f32> = page_cols
            .iter()
            .map(|c| base_width(*c, cols).max(text_w(&cols.name(*c)) + HEADER_EXTRA))
            .collect();
        let longest = entries.iter().map(|e| text_w(&e.title)).fold(0.0, f32::max);
        let book_want = COVER_SIZE.0 + 6.0 + 2.0 * gap + longest + 4.0;
        let book_floor = book_want.clamp(BOOK_MIN, BOOK_FLOOR_MAX);
        let pinned = READ_COL + MARK_COL + REMOVE_COL + 4.0 * gap + 12.0;
        let avail = ui.available_width();
        let room = (avail - pinned - book_floor - gap).max(60.0);
        let pages = lc::pack_pages(&widths, room, gap);
        let page_w = |r: &std::ops::Range<usize>| {
            widths[r.clone()].iter().sum::<f32>() + gap * r.len().saturating_sub(1) as f32
        };
        let widest = pages.iter().map(page_w).fold(0.0, f32::max);
        let book_w = book_want.min(avail - pinned - widest - gap).max(BOOK_MIN);
        let page = self
            .columns
            .page_at
            .and_then(|a| page_cols.iter().position(|c| *c == a))
            .and_then(|i| pages.iter().position(|p| p.contains(&i)))
            .unwrap_or(0);
        let range = pages[page].clone();

        // The row under the header: filters set, and the pager.
        let mut clear_all = false;
        if !cols.filters.is_empty() || pages.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                for (c, f) in &cols.filters {
                    let name = cols.name(*c);
                    let r = ui
                        .add(
                            egui::Button::new(
                                RichText::new(describe(*c, &name, f)).color(palette::accent()),
                            )
                            .small(),
                        )
                        .on_hover_text("Click to change this filter");
                    let mut nf = f.clone();
                    let kind = c.filter_kind(&cols.fields);
                    let choices = if kind == FilterKind::Choice {
                        data.choices(*c)
                    } else {
                        Vec::new()
                    };
                    let span = span(&data, *c);
                    let mut changed = false;
                    egui::Popup::from_toggle_button_response(&r)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .show(|ui| {
                            changed = filter_editor(ui, *c, kind, &mut nf, &choices, span);
                        });
                    if changed {
                        acts.push(Act::Filter(*c, nf));
                    }
                    if ui
                        .small_button(icons::glyph(ui.ctx(), X, false))
                        .on_hover_text("Remove this filter")
                        .clicked()
                    {
                        acts.push(Act::Filter(*c, Filter::default()));
                    }
                    ui.add_space(6.0);
                }
                if cols.filters.len() > 1 && ui.small_button("Clear filters").clicked() {
                    clear_all = true;
                }
                if pages.len() > 1 {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                page + 1 < pages.len(),
                                egui::Button::new(icons::glyph(ui.ctx(), CARET_RIGHT, false)),
                            )
                            .on_hover_text("The next columns")
                            .clicked()
                        {
                            acts.push(Act::Page(page_cols[pages[page + 1].start]));
                        }
                        ui.label(format!("{} / {}", page + 1, pages.len()));
                        if ui
                            .add_enabled(
                                page > 0,
                                egui::Button::new(icons::glyph(ui.ctx(), CARET_LEFT, false)),
                            )
                            .on_hover_text("The columns before")
                            .clicked()
                        {
                            acts.push(Act::Page(page_cols[pages[page - 1].start]));
                        }
                        let on_page = page_cols[range.clone()].contains(&col);
                        if !on_page && !matches!(col, HistCol::Title | HistCol::Read) {
                            ui.label(icons::rich(
                                ui,
                                RichText::new(format!(
                                    "Sorted by {} {}",
                                    cols.name(col),
                                    if desc { CARET_DOWN } else { CARET_UP }
                                ))
                                .weak(),
                            ));
                        }
                    });
                }
            });
        }

        if rows.is_empty() {
            ui.label(RichText::new("No book matches the search and filters.").weak());
            if !cols.filters.is_empty() && ui.button("Clear filters").clicked() {
                clear_all = true;
            }
        } else {
            let shown: Vec<(HistCol, f32)> =
                range.clone().map(|i| (page_cols[i], widths[i])).collect();
            let covers = &mut self.covers;
            let exists = &self.exists;
            let hist = &self.history;
            let du = &self.du;
            let edit = &mut self.columns.edit;
            let mut tb = TableBuilder::new(ui)
                // Cells otherwise inherit the top-down layout: text sits at the row top.
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .striped(true)
                .column(Column::exact(book_w).clip(true))
                .column(Column::exact(READ_COL));
            for (_, w) in &shown {
                tb = tb.column(Column::exact(*w).clip(true));
            }
            tb.column(Column::exact(MARK_COL))
                .column(Column::remainder().at_least(REMOVE_COL))
                .header(24.0, |mut h| {
                    h.col(|ui| {
                        if let Some(s) = sort_header(ui, "Book", lc::hover(HistCol::Title), HistCol::Title, (col, desc), false)
                        {
                            acts.push(Act::Sort(s));
                        }
                    });
                    h.col(|_| {});
                    for (c, _) in shown.iter().copied().chain([(HistCol::Read, MARK_COL)]) {
                        h.col(|ui| {
                            let name = cols.name(c);
                            let hover = match c {
                                HistCol::Field(_) => {
                                    let ty = cols.field(match c {
                                        HistCol::Field(id) => id,
                                        _ => 0,
                                    });
                                    format!(
                                        "Your own column ({}). Click a cell to change it.",
                                        ty.map_or("?", |f| f.ty.label().trim_end_matches(['\u{e46a}', ' ']))
                                    )
                                }
                                c => lc::hover(c).to_owned(),
                            };
                            let label = if c == HistCol::Read { "Read" } else { name.as_str() };
                            if let Some(s) = sort_header(ui, label, &hover, c, (col, desc), c.first_desc(&cols.fields)) {
                                acts.push(Act::Sort(s));
                            }
                            let current = cols.filter(c);
                            let r = filter_button(ui, current.is_some(), label).on_hover_text("Filter by this column");
                            let mut nf = current.cloned().unwrap_or_default();
                            let kind = c.filter_kind(&cols.fields);
                            let mut changed = false;
                            egui::Popup::from_toggle_button_response(&r)
                                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                                .show(|ui| {
                                    ui.strong(&name);
                                    let choices = if kind == FilterKind::Choice { data.choices(c) } else { Vec::new() };
                                    changed = filter_editor(ui, c, kind, &mut nf, &choices, span(&data, c));
                                });
                            if changed {
                                acts.push(Act::Filter(c, nf));
                            }
                        });
                    }
                    h.col(|_| {});
                })
                .body(|body| {
                    body.rows(COVER_ROW, rows.len(), |mut row| {
                        let i = rows[row.index()];
                        let e = &entries[i];
                        let s = &e.stats;
                        row.col(|ui| {
                            let tex = covers
                                .entry(e.id.clone())
                                .or_insert_with(|| load_cover(ctx, &hist.cover(&e.id)));
                            book_cell(ui, e, tex.as_ref(), exists.get(&e.path), du.library_tag(&e.path, data.now), &mut acts);
                        });
                        row.col(|ui| {
                            // As on the Overview: the reader takes EPUBs and TXTs.
                            if crate::reader::opens(&s.format)
                                && ui
                                    .add_enabled(exists.get(&e.path), egui::Button::new(icons::rich(ui, format!("{BOOK_OPEN} Read"))))
                                    .on_hover_text("Read the book in the reader")
                                    .on_disabled_hover_text("The file is no longer there")
                                    .clicked()
                            {
                                acts.push(Act::Read(e.path.clone(), e.id.clone(), s.variant == "Traditional"));
                            }
                        });
                        for (c, w) in &shown {
                            row.col(|ui| cell(ui, *c, *w, i, &data, edit, &mut acts));
                        }
                        row.col(|ui| match data.read_at.get(&e.id) {
                            Some(&t) => {
                                if ui
                                    .button(icons::glyph(ui.ctx(), CHECK, true).color(palette::good()))
                                    .on_hover_text(format!(
                                        "Marked read {} ({}). Click to change how many of its words count as known, or to unmark it.",
                                        history::age(t, data.now),
                                        history::date_time(t)
                                    ))
                                    .clicked()
                                {
                                    acts.push(Act::MarkRead(e.id.clone()));
                                }
                            }
                            None => {
                                let has_words = exists.get(&hist.profile(&e.id));
                                if ui
                                    .add_enabled(has_words, egui::Button::new(icons::glyph(ui.ctx(), CHECK, false).weak()))
                                    .on_hover_text("Mark read: count its most frequent words as known and take it off the reading path")
                                    .on_disabled_hover_text(icons::rich(ui, format!("No word data yet: open the book, or {ARROW_CLOCKWISE} Recalculate library")))
                                    .clicked()
                                {
                                    acts.push(Act::MarkRead(e.id.clone()));
                                }
                            }
                        });
                        row.col(|ui| {
                            // Apart from ✔, so as not to be hit instead of it.
                            ui.add_space(6.0);
                            if ui
                                .small_button(icons::glyph(ui.ctx(), X, false))
                                .on_hover_text(
                                    "Remove from the Library, with hanstat's copy of the book (the file you added it from is not touched); it can be put back for a few seconds",
                                )
                                .clicked()
                            {
                                acts.push(Act::Remove(e.id.clone()));
                            }
                        });
                    });
                });
        }

        if clear_all {
            self.columns.cols.filters.clear();
            self.columns.save();
        }
        let mut save = false;
        for a in acts {
            match a {
                Act::Open(p) => self.open(p, ctx),
                Act::Read(path, id, traditional) => {
                    match crate::reader::Reader::open(
                        &path,
                        Some(&id),
                        traditional,
                        ctx,
                        &mut self.reader_dictionary,
                    ) {
                        Ok(r) => self.reader = Some(r),
                        Err(e) => self.set_error("reader", e),
                    }
                }
                Act::Remove(id) => self.remove_book(&id, ctx.input(|i| i.time)),
                Act::MarkRead(id) => self.open_read_dialog(&id),
                Act::Sort(s) => self.history_sort = s,
                Act::Filter(c, f) => {
                    self.columns.cols.set_filter(c, f);
                    save = true;
                }
                Act::Value(book, field, v) => {
                    self.columns.cols.set_value(&book, field, v);
                    save = true;
                }
                Act::Edit(book, field, text) => {
                    self.columns.edit = Some((book, field, text, false))
                }
                Act::Page(c) => self.columns.page_at = Some(c),
            }
        }
        if save {
            self.columns.save();
        }
    }

    /// The Columns window: order, show, add and remove columns.
    fn columns_window(&mut self, ctx: &egui::Context) {
        let style_on = self.style_on();
        let mut open = true;
        let mut changed = false;
        let st = &mut self.columns;
        egui::Window::new("Library columns")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.label(icons::rich(ui, RichText::new(format!("Book and {BOOK_OPEN} Read are always first; {CHECK} Read and {X} always last.")).weak()));
                ui.add_space(4.0);
                let cols = &mut st.cols;
                let n = cols.order.len();
                let mut moved: Option<(usize, usize)> = None;
                let mut remove: Option<HistCol> = None;
                let mut delete: Option<u32> = None;
                let mut rename_done = false;
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    for i in 0..n {
                        let (c, mut shown) = cols.order[i];
                        let row = ui.horizontal(|ui| {
                            ui.dnd_drag_source(egui::Id::unique(("lib-col", i)), i, |ui| {
                                ui.label(icons::glyph(ui.ctx(), DOTS_SIX_VERTICAL, false).weak());
                            })
                            .response
                            .on_hover_text("Drag to move");
                            if ui.add_enabled(i > 0, egui::Button::new(icons::glyph(ui.ctx(), CARET_UP, false)).small()).on_hover_text("Move up").clicked() {
                                moved = Some((i, i - 1));
                            }
                            if ui
                                .add_enabled(i + 1 < n, egui::Button::new(icons::glyph(ui.ctx(), CARET_DOWN, false)).small())
                                .on_hover_text("Move down")
                                .clicked()
                            {
                                moved = Some((i, i + 1));
                            }
                            let renaming = matches!(&st.rename, Some((id, _)) if HistCol::Field(*id) == c);
                            if renaming {
                                if let Some((id, text)) = &mut st.rename {
                                    let r = ui.add(egui::TextEdit::singleline(text).desired_width(150.0));
                                    let ok = cols.name_free(text, Some(*id));
                                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                    if (ui.add_enabled(ok, egui::Button::new(icons::glyph(ui.ctx(), CHECK, false))).clicked() || (enter && ok))
                                        && let Some(f) = cols.fields.iter_mut().find(|f| f.id == *id)
                                    {
                                        f.name = text.trim().to_owned();
                                        rename_done = true;
                                    }
                                    if ui.small_button("Cancel").clicked() {
                                        rename_done = true;
                                    }
                                }
                            } else {
                                let name = cols.name(c);
                                if ui.checkbox(&mut shown, name).changed() {
                                    cols.order[i].1 = shown;
                                    changed = true;
                                }
                                let (tag, hover) = match c {
                                    HistCol::Field(id) => (
                                        cols.field(id).map_or("?", |f| f.ty.label()).to_owned(),
                                        "Your own column".to_owned(),
                                    ),
                                    c if c.is_stat() => ("stat".to_owned(), lc::hover(c).to_owned()),
                                    HistCol::Style if !style_on => (
                                        format!("default · off in {GEAR}"),
                                        format!("Shown while writing-style rating is on ({GEAR} Settings)"),
                                    ),
                                    c => ("default".to_owned(), lc::hover(c).to_owned()),
                                };
                                ui.label(icons::rich(ui, RichText::new(tag).small().weak())).on_hover_text(icons::rich(ui, hover));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match c {
                                    HistCol::Field(id) => {
                                        if ui.small_button(icons::glyph(ui.ctx(), TRASH, false)).on_hover_text("Delete this column and its values (Undo for a few seconds)").clicked() {
                                            delete = Some(id);
                                        }
                                        if ui.small_button(icons::glyph(ui.ctx(), PENCIL, false)).on_hover_text("Rename").clicked() {
                                            st.rename = Some((id, cols.name(c)));
                                        }
                                    }
                                    c if c.is_stat()
                                        && ui
                                            .small_button(icons::glyph(ui.ctx(), X, false))
                                            .on_hover_text("Remove (it can be added again)")
                                            .clicked() =>
                                    {
                                        remove = Some(c);
                                    }
                                    _ => {}
                                });
                            }
                        });
                        let r = row.response;
                        if let (Some(p), Some(_)) = (ui.input(|i| i.pointer.interact_pos()), r.dnd_hover_payload::<usize>()) {
                            let above = p.y < r.rect.center().y;
                            let y = if above { r.rect.top() } else { r.rect.bottom() };
                            ui.painter().hline(r.rect.x_range(), y, egui::Stroke::new(2.0, palette::accent()));
                            if let Some(from) = r.dnd_release_payload::<usize>() {
                                let mut to = if above { i } else { i + 1 };
                                if to > *from {
                                    to -= 1;
                                }
                                moved = Some((*from, to));
                            }
                        }
                    }
                });
                if let Some((a, b)) = moved {
                    cols.move_to(a, b);
                    changed = true;
                }
                if let Some(c) = remove {
                    cols.remove_stat(c);
                    changed = true;
                }
                if rename_done {
                    st.rename = None;
                    changed = true;
                }
                if let Some(id) = delete
                    && let Some(d) = cols.delete_field(id)
                {
                    // One at a time: the one before can't be put back now.
                    if st.deleted.take().is_some() {
                        changed = true;
                    }
                    st.deleted = Some((d, ui.input(|i| i.time) + UNDO_SECS));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.menu_button("＋ Add column", |ui| {
                        let mut any = false;
                        for c in STATS {
                            if cols.order.iter().any(|(o, _)| *o == c) {
                                continue;
                            }
                            any = true;
                            if ui.button(lc::name(c)).on_hover_text(lc::hover(c)).clicked() {
                                cols.add_stat(c);
                                changed = true;
                                ui.close();
                            }
                        }
                        if any {
                            ui.separator();
                        }
                        if ui.button("Your own…").on_hover_text("A column you name, holding what you enter for each book").clicked() {
                            st.new_field = Some((String::new(), FieldType::Text));
                            ui.close();
                        }
                    });
                    if ui
                        .button("Reset to default")
                        .on_hover_text("The default columns in their order, shown; stat columns removed; your own columns kept, hidden, with their values")
                        .clicked()
                    {
                        cols.reset();
                        changed = true;
                    }
                });
                let mut done = false;
                if let Some((name, ty)) = &mut st.new_field {
                    ui.separator();
                    ui.strong("New column");
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        let r = ui.add(egui::TextEdit::singleline(name).desired_width(160.0).hint_text("Author"));
                        if r.gained_focus() || name.is_empty() {
                            r.request_focus();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Type");
                        egui::ComboBox::from_id_salt("new-col-type")
                            .selected_text(icons::rich(ui, ty.label()))
                            .show_ui(ui, |ui| {
                                for t in FieldType::ALL {
                                    ui.selectable_value(ty, t, icons::rich(ui, t.label()));
                                }
                            });
                    });
                    let ok = cols.name_free(name, None);
                    if !ok && !name.trim().is_empty() {
                        ui.colored_label(palette::warn(), "A column has that name already.");
                    }
                    ui.horizontal(|ui| {
                        if ui.add_enabled(ok, egui::Button::new("Add")).clicked() {
                            cols.add_field(name, *ty);
                            changed = true;
                            done = true;
                        }
                        if ui.button("Cancel").clicked() {
                            done = true;
                        }
                    });
                }
                if done {
                    st.new_field = None;
                }
            });
        if changed {
            self.columns.save();
        }
        if !open {
            self.columns.window_open = false;
            self.columns.new_field = None;
            self.columns.rename = None;
        }
    }
}

/// The smallest and largest value of a Range column, to start bounds at.
fn span(data: &Data, c: HistCol) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for i in 0..data.entries.len() {
        if let Val::Num(x) = data.val(c, i) {
            lo = lo.min(x);
            hi = hi.max(x);
        }
    }
    if lo.is_finite() {
        ((lo * 10.0).floor() / 10.0, (hi * 10.0).ceil() / 10.0)
    } else {
        (0.0, 100.0)
    }
}

/// A column's width before its header is measured.
fn base_width(c: HistCol, cols: &Columns) -> f32 {
    match c {
        HistCol::Style => 140.0,
        HistCol::Series => 170.0,
        HistCol::Difficulty | HistCol::Progress => 110.0,
        HistCol::Field(id) => match cols.field(id).map(|f| f.ty) {
            Some(FieldType::Text) => 160.0,
            Some(FieldType::Date) => 150.0,
            Some(FieldType::Rating) => 110.0,
            _ => 90.0,
        },
        _ => 100.0,
    }
}

/// The Book cell: the cover, then the title (a link that opens the book).
fn book_cell(
    ui: &mut egui::Ui,
    e: &history::Entry,
    tex: Option<&egui::TextureHandle>,
    there: bool,
    du_tag: Option<(String, u32)>,
    acts: &mut Vec<Act>,
) {
    let s = &e.stats;
    let (w, h) = COVER_SIZE;
    match tex {
        Some(t) => {
            // Fit the box, keeping the cover's proportions.
            let [tw, th] = t.size().map(|x| x as f32);
            let k = (w / tw).min(h / th);
            ui.allocate_ui(egui::vec2(w, h), |ui| {
                ui.centered_and_justified(|ui| {
                    ui.add(
                        egui::Image::new(t)
                            .fit_to_exact_size(egui::vec2(tw * k, th * k))
                            .corner_radius(2.0),
                    )
                    .on_hover_ui(|ui| {
                        let k = 2.5 * k;
                        ui.add(egui::Image::new(t).fit_to_exact_size(egui::vec2(tw * k, th * k)));
                    });
                });
            });
        }
        None => {
            ui.allocate_space(egui::vec2(w, h));
        }
    }
    ui.add_space(6.0);
    let mut hover = format!(
        "{}\n{} · {} · {} of {} sections included",
        e.path.display(),
        s.format,
        s.segmenter,
        s.sections_included,
        s.sections_total
    );
    if let Some(k) = &s.known {
        hover.push_str(&format!(
            "\nKnown from {}: {:.1}% of unique words ({} not known)",
            k.source,
            k.unique_words,
            fmt_int(k.unknown_words)
        ));
        hover.push_str(&format!(", {:.1}% of unique characters", k.unique_chars));
    }
    if s.words_for_98 > 0 {
        hover.push_str(&format!(
            "\n{} most frequent words cover 98% of the text",
            fmt_int(s.words_for_98)
        ));
    }
    ui.vertical(|ui| {
        if du_tag.is_some() {
            ui.add_space(6.0);
        }
        if there {
            if ui.link(&e.title).on_hover_text(hover).clicked() {
                acts.push(Act::Open(e.path.clone()));
            }
        } else {
            ui.label(RichText::new(&e.title).weak())
                .on_hover_text(format!("{hover}\n\nThe file is no longer there."));
        }
        if let Some((progress, new)) = du_tag {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(
                    RichText::new("DuChinese")
                        .small()
                        .color(palette::highlight()),
                )
                .on_hover_text("Imported from DuChinese: new chapters are added as they come out");
                ui.label(
                    RichText::new(format!("· {progress}"))
                        .small()
                        .color(palette::weak()),
                );
                if new > 0 {
                    ui.label(
                        RichText::new(format!("· {new} new"))
                            .small()
                            .color(palette::accent()),
                    );
                }
            });
        }
    });
}

/// A data cell.
fn cell(
    ui: &mut egui::Ui,
    c: HistCol,
    w: f32,
    i: usize,
    data: &Data,
    edit: &mut Option<(String, u32, String, bool)>,
    acts: &mut Vec<Act>,
) {
    let e = &data.entries[i];
    let s = &e.stats;
    let v = data.val(c, i);
    match c {
        HistCol::Language => {
            let variant = match s.variant.as_str() {
                "Simplified" => Some(Variant::Simplified),
                "Traditional" => Some(Variant::Traditional),
                "Mixed" => Some(Variant::Mixed),
                _ => None,
            };
            ui.label(script_badge(variant).0);
        }
        HistCol::Known => match &s.known {
            Some(k) => {
                ui.label(format!("{:.1}%", k.running_words))
                    .on_hover_text(format!("Known from {}", k.source));
            }
            None => {
                ui.label(dash())
                    .on_hover_text("Nothing was marked known: Anki was off and no levels or word lists were ticked");
            }
        },
        HistCol::Difficulty => match &s.known {
            Some(k) => {
                let off = data.style_off.get(e.id.as_str()).copied().unwrap_or(0.0);
                let counted = k.running_words - off;
                let d = Difficulty::from_percent(counted);
                let t = ui.label(RichText::new(d.label()).color(difficulty_color(d)));
                if off.abs() >= 0.05 {
                    t.on_hover_text(format!(
                        "{}\n{:.1}% known, {} {:.1} points for its writing style: counts as {counted:.1}%",
                        d.description(),
                        k.running_words,
                        if off > 0.0 { "less" } else { "plus" },
                        off.abs()
                    ));
                } else {
                    t.on_hover_text(d.description());
                }
            }
            None => {
                ui.label(dash());
            }
        },
        HistCol::Style => match data.style_scores.get(e.id.as_str()) {
            Some(&x) => {
                ui.label(format!(
                    "{} · {}",
                    data.style_scale.pct(x),
                    hanstat::style::label(x)
                ))
                .on_hover_text(data.style_scale.hover(x));
            }
            None => {
                ui.label(dash()).on_hover_text(
                    data.style_skipped
                        .get(&e.id)
                        .map_or("Not rated yet", String::as_str),
                );
            }
        },
        HistCol::Time | HistCol::TimeRead => match v {
            Val::Num(h) => {
                ui.label(hm(h));
            }
            _ => {
                ui.label(dash());
            }
        },
        HistCol::Analysed | HistCol::FirstRead | HistCol::Finished => match v {
            Val::Time(t) => {
                ui.label(history::age(t as u64, data.now))
                    .on_hover_text(history::date_time(t as u64));
            }
            _ => {
                ui.label(dash());
            }
        },
        HistCol::LastRead => match v {
            Val::Day(d) => {
                let ago = today() - d;
                ui.label(lc::date_label(d)).on_hover_text(match ago {
                    0 => "Today".to_owned(),
                    1 => "Yesterday".to_owned(),
                    n => format!("{n} days ago"),
                });
            }
            _ => {
                ui.label(dash());
            }
        },
        HistCol::Progress => progress_cell(ui, w, i, data),
        HistCol::KnownUnique | HistCol::KnownChars => match v {
            Val::Num(x) => {
                ui.label(format!("{x:.1}%"));
            }
            _ => {
                ui.label(dash());
            }
        },
        HistCol::Speed => match v {
            Val::Num(x) => {
                ui.label(format!("{x:.0} cpm"));
            }
            _ => {
                ui.label(dash());
            }
        },
        HistCol::Sections => {
            ui.label(format!("{} / {}", s.sections_included, s.sections_total));
        }
        HistCol::Series => match data.series.get(e.id.as_str()) {
            Some((_, _, meta)) => {
                let Val::Text(t) = v else { return };
                ui.label(t).on_hover_text(if *meta {
                    "From the book's metadata (as calibre writes it)"
                } else {
                    "From the title"
                });
            }
            None => {
                ui.label(dash());
            }
        },
        HistCol::Field(id) => field_cell(ui, id, w, i, data, edit, acts),
        _ => match v {
            Val::Num(x) => {
                ui.label(fmt_int(x as u64));
            }
            Val::Choice(l, _) | Val::Text(l) => {
                ui.label(l);
            }
            _ => {
                ui.label(dash());
            }
        },
    }
}

/// "42%" with a thin bar under it.
fn progress_cell(ui: &mut egui::Ui, w: f32, i: usize, data: &Data) {
    let e = &data.entries[i];
    let place = data.place(&e.id);
    let Some(p) = data.progress(i) else {
        if place.is_some() && crate::reader::opens(&e.stats.format) {
            ui.label(RichText::new("…").weak())
                .on_hover_text("Working out the chapters' lengths");
        } else {
            ui.label(dash()).on_hover_text("Not opened in the reader");
        }
        return;
    };
    let mut hover = Vec::new();
    if let Some(&t) = data.read_at.get(&e.id) {
        hover.push(format!("Marked read {}", history::age(t, data.now)));
    } else if let Some(t) = data.reading.get(&e.id).and_then(|r| r.finished) {
        hover.push(format!(
            "Finished in the reader {}",
            history::age(t, data.now)
        ));
    }
    if let Some(at) = place {
        if let Some(ch) = chapter_lengths::get(&e.id) {
            hover.push(format!(
                "Chapter {} of {}",
                (at.spine as usize + 1).min(ch.len()),
                ch.len()
            ));
        }
        if at.saved > 0 {
            hover.push(format!("left {}", history::age(at.saved, data.now)));
        }
    }
    ui.vertical(|ui| {
        ui.add_space(10.0);
        ui.label(if p > 0.0 && p < 1.0 {
            "<1%".to_owned()
        } else {
            format!("{p:.0}%")
        });
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2((w - 16.0).max(30.0), 4.0), egui::Sense::hover());
        crate::home::progress_bar(ui, rect, (p / 100.0) as f32);
    })
    .response
    .on_hover_text(hover.join(" · "));
}

/// A user column's cell, edited in place.
fn field_cell(
    ui: &mut egui::Ui,
    id: u32,
    w: f32,
    i: usize,
    data: &Data,
    edit: &mut Option<(String, u32, String, bool)>,
    acts: &mut Vec<Act>,
) {
    let e = &data.entries[i];
    let Some(f) = data.cols.field(id) else { return };
    let value = data.cols.value(&e.id, id);
    match f.ty {
        FieldType::Text | FieldType::Number => {
            let editing = matches!(edit, Some((b, fid, _, _)) if *b == e.id && *fid == id);
            if editing {
                let Some((_, _, text, focused)) = edit else {
                    return;
                };
                let r = ui.add(egui::TextEdit::singleline(text).desired_width(w - 10.0));
                if !*focused {
                    r.request_focus();
                    *focused = true;
                }
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                let bad = f.ty == FieldType::Number
                    && !text.trim().is_empty()
                    && parse_number(text).is_none();
                if bad {
                    ui.painter().rect_stroke(
                        r.rect.expand(1.0),
                        3.0,
                        egui::Stroke::new(1.5, palette::warn()),
                        egui::StrokeKind::Outside,
                    );
                }
                if esc {
                    *edit = None;
                } else if r.lost_focus() {
                    let t = text.trim();
                    let v = match f.ty {
                        _ if t.is_empty() => Some(None),
                        FieldType::Number => parse_number(t).map(|x| Some(Value::Number(x))),
                        _ => Some(Some(Value::Text(t.to_owned()))),
                    };
                    if let Some(v) = v {
                        acts.push(Act::Value(e.id.clone(), id, v));
                    }
                    *edit = None;
                }
                return;
            }
            let shown = match value {
                Some(Value::Text(t)) => RichText::new(t),
                Some(Value::Number(x)) => RichText::new(fmt_number(*x)),
                _ => dash(),
            };
            let r = ui
                .add(
                    egui::Label::new(shown)
                        .truncate()
                        .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::Text);
            let r = match value {
                Some(Value::Text(t)) => r.on_hover_text(format!("{t}\n\nClick to change")),
                _ => r.on_hover_text("Click to change"),
            };
            if r.clicked() {
                let text = match value {
                    Some(Value::Text(t)) => t.clone(),
                    Some(Value::Number(x)) => fmt_number(*x),
                    _ => String::new(),
                };
                acts.push(Act::Edit(e.id.clone(), id, text));
            }
        }
        FieldType::YesNo => {
            let mut yes = matches!(value, Some(Value::Yes(true)));
            if ui.checkbox(&mut yes, "").changed() {
                acts.push(Act::Value(
                    e.id.clone(),
                    id,
                    yes.then_some(Value::Yes(true)),
                ));
            }
        }
        FieldType::Rating => {
            let n = match value {
                Some(Value::Stars(n)) => *n,
                _ => 0,
            };
            ui.spacing_mut().item_spacing.x = 1.0;
            for k in 1..=5u8 {
                let on = k <= n;
                let r = ui
                    .add(
                        egui::Label::new(icons::glyph(ui.ctx(), STAR, on).color(if on {
                            palette::highlight()
                        } else {
                            palette::weak()
                        }))
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(if k == n {
                        "Click again to clear"
                    } else {
                        "Rate"
                    });
                if r.clicked() {
                    acts.push(Act::Value(
                        e.id.clone(),
                        id,
                        (k != n).then_some(Value::Stars(k)),
                    ));
                }
            }
        }
        FieldType::Date => {
            let mut day = match value {
                Some(Value::Date(d)) => Some(*d),
                _ => None,
            };
            let salt = format!("libdate-{}-{id}", e.id);
            if day_picker(ui, &mut day, &salt) {
                acts.push(Act::Value(e.id.clone(), id, day.map(Value::Date)));
            }
        }
    }
}

/// A number typed in a cell: `1,234.5` or `1234,5` both work.
fn parse_number(t: &str) -> Option<f64> {
    let t = t.trim().replace([' ', '_'], "");
    let t = if t.contains('.') {
        t.replace(',', "")
    } else {
        t.replace(',', ".")
    };
    t.parse::<f64>().ok().filter(|x| x.is_finite())
}

fn fmt_number(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_typed() {
        assert_eq!(parse_number("1,234.5"), Some(1234.5));
        assert_eq!(parse_number("12,5"), Some(12.5));
        assert_eq!(parse_number(" 300 "), Some(300.0));
        assert_eq!(parse_number("abc"), None);
        assert_eq!(fmt_number(300.0), "300");
        assert_eq!(fmt_number(2.5), "2.5");
    }

    #[test]
    fn dates_convert_both_ways() {
        for d in [-1, 0, 19_782, 20_000] {
            assert_eq!(from_date(to_date(d)), d);
        }
        let leap = rs::days_from_civil(2024, 2, 29) as i32;
        assert_eq!(to_date(leap), jiff::civil::date(2024, 2, 29));
    }

    #[test]
    fn chips_say_what_filters_do() {
        let f = Filter {
            min: Some(90.0),
            blanks: false,
            ..Filter::default()
        };
        assert_eq!(describe(HistCol::Known, "Known", &f), "Known: ≥ 90%");
        let not_read = Filter {
            values: false,
            ..Filter::default()
        };
        assert_eq!(describe(HistCol::Read, "Read", &not_read), "Not read");
    }
}
