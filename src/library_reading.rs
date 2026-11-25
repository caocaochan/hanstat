//! Reading stats in the Library: the Reading section of Library Stats and
//! the "Read so far" rows of a book's Overview. The figures are the
//! reader's ([`crate::reading_stats`]), drawn with the Library's tiles,
//! cards and charts.

use crate::charts::{self, Bar, CalDay};
use crate::library_stats::{Tile, card, tile_grid};
use crate::reader::stats_mins as mins;
use crate::reading_stats::{self as rs, Day, Summary};
use crate::{App, fmt_int, palette};
use eframe::egui::{self, RichText};
use std::collections::BTreeMap;
use std::sync::Arc;

const CHART_HEIGHT: f32 = 200.0;
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

#[derive(Clone, Copy, Default, PartialEq)]
enum Measure {
    #[default]
    Time,
    Chars,
}

/// What the Reading section covers: everything, or one day, week (Monday
/// to Sunday), month or year.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Unit {
    #[default]
    All,
    Year,
    Month,
    Week,
    Day,
}

impl Unit {
    fn name(self) -> &'static str {
        match self {
            Unit::All => "all time",
            Unit::Year => "year",
            Unit::Month => "month",
            Unit::Week => "week",
            Unit::Day => "day",
        }
    }
}

/// Reading days `start..end` (day numbers) of a [`Unit`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct Period {
    unit: Unit,
    start: i32,
    end: i32,
}

fn month_start(y: i64, m: i64) -> i32 {
    let (y, m) = (y + (m - 1).div_euclid(12), (m - 1).rem_euclid(12) + 1);
    rs::days_from_civil(y, m, 1) as i32
}

impl Period {
    /// The `unit` holding day `anchor`; `None` for all time.
    fn of(unit: Unit, anchor: i32) -> Option<Period> {
        let (y, m, _) = rs::civil(anchor as i64);
        let (start, end) = match unit {
            Unit::All => return None,
            Unit::Day => (anchor, anchor + 1),
            Unit::Week => {
                let s = anchor - rs::weekday(anchor) as i32;
                (s, s + 7)
            }
            Unit::Month => (month_start(y, m as i64), month_start(y, m as i64 + 1)),
            Unit::Year => (month_start(y, 1), month_start(y + 1, 1)),
        };
        Some(Period { unit, start, end })
    }

    /// The one before (`dir` −1) or after (+1).
    fn step(self, dir: i32) -> Period {
        let (y, m, _) = rs::civil(self.start as i64);
        let anchor = match self.unit {
            Unit::All => self.start,
            Unit::Day => self.start + dir,
            Unit::Week => self.start + 7 * dir,
            Unit::Month => month_start(y, m as i64 + dir as i64),
            Unit::Year => month_start(y + dir as i64, 1),
        };
        Period::of(self.unit, anchor).unwrap_or(self)
    }

    fn contains(self, day: i32) -> bool {
        (self.start..self.end).contains(&day)
    }

    /// "Mon 23 Nov 2026", "17–23 Nov 2026", "November 2026", "2026".
    fn label(self) -> String {
        let (y, m, d) = rs::civil(self.start as i64);
        match self.unit {
            Unit::All => "All time".to_owned(),
            Unit::Day => rs::long_date(self.start),
            Unit::Week => {
                let (y2, m2, d2) = rs::civil(self.end as i64 - 1);
                let mon = |m: u32| MONTHS[m as usize - 1];
                if y != y2 {
                    format!("{d} {} {y} – {d2} {} {y2}", mon(m), mon(m2))
                } else if m != m2 {
                    format!("{d} {} – {d2} {} {y}", mon(m), mon(m2))
                } else {
                    format!("{d}–{d2} {} {y}", mon(m))
                }
            }
            Unit::Month => format!("{} {y}", MONTH_NAMES[m as usize - 1]),
            Unit::Year => y.to_string(),
        }
    }

    /// "on" a day, "in" anything longer.
    fn prep(self) -> &'static str {
        if self.unit == Unit::Day { "on" } else { "in" }
    }

    /// "this week", "today".
    fn current_name(self) -> &'static str {
        match self.unit {
            Unit::Day => "Today",
            Unit::Week => "This week",
            Unit::Month => "This month",
            Unit::Year => "This year",
            Unit::All => "All time",
        }
    }
}

/// What the Reading section shows, and its summary, made again only when
/// the stats or the day change.
#[derive(Default)]
pub(crate) struct ReadingView {
    year: Option<i64>,
    long: bool,
    measure: Measure,
    unit: Unit,
    /// A day in the period shown; `None`: today.
    anchor: Option<i32>,
    cache: Option<(u64, i32, Arc<Summary>, rs::TrackerSettings)>,
    remove: Option<(String, String)>,
}

impl ReadingView {
    fn summary(&mut self) -> (Arc<Summary>, rs::TrackerSettings) {
        let now = rs::now();
        let rev = rs::revision();
        let today = rs::with(|s| rs::local_day(now, s.settings.day_start_hour));
        if let Some((r, t, s, set)) = &self.cache
            && *r == rev
            && *t == today
        {
            return (s.clone(), set.clone());
        }
        let (sum, set) = rs::with(|s| (Arc::new(s.summary(today)), s.settings.clone()));
        self.cache = Some((rev, today, sum.clone(), set.clone()));
        (sum, set)
    }
}

fn speed(d: &Day) -> Option<f64> {
    (d.read_secs >= 60.0 && d.read_chars > 0).then(|| d.read_chars as f64 / (d.read_secs / 60.0))
}

fn plural(n: impl Into<u64>, one: &str) -> String {
    let n = n.into();
    format!("{} {one}{}", fmt_int(n), if n == 1 { "" } else { "s" })
}

/// The calendar's shades: no reading, then light to dark (blue).
pub(crate) fn calendar_shades() -> [egui::Color32; 5] {
    let c = palette::cat();
    let base = c.base;
    [
        c.surface0,
        palette::mix(c.blue, base, 0.35),
        palette::mix(c.blue, base, 0.55),
        palette::mix(c.blue, base, 0.78),
        c.blue,
    ]
}

/// A day's shade in [`calendar_shades`]: under 15 min, 15–30, 30–60, an
/// hour or more.
pub(crate) fn day_level(v: &Day) -> u8 {
    let m = v.read_secs / 60.0;
    if m < 1.0 && v.read_chars == 0 {
        0
    } else if m < 15.0 {
        1
    } else if m < 30.0 {
        2
    } else if m < 60.0 {
        3
    } else {
        4
    }
}

pub(crate) fn day_hover(day: i32, d: &Day, goal: bool) -> String {
    span_hover(rs::long_date(day), d, goal)
}

fn span_hover(mut t: String, d: &Day, goal: bool) -> String {
    if d.read_secs >= 1.0 || d.read_chars > 0 {
        t.push_str(&format!(
            "\n{} reading, {} characters",
            mins(d.read_secs),
            fmt_int(d.read_chars)
        ));
        if let Some(v) = speed(d) {
            t.push_str(&format!(" ({v:.0}/min)"));
        }
    } else {
        t.push_str("\nNo reading");
    }
    if d.listen_secs >= 1.0 {
        t.push_str(&format!(
            "\n{} listening, {} characters",
            mins(d.listen_secs),
            fmt_int(d.listen_chars)
        ));
    }
    if goal {
        t.push_str("\nGoal met");
    }
    t
}

/// The longest run of days in `a..b` that pass `ok`.
fn longest_run(a: i32, b: i32, ok: impl Fn(i32) -> bool) -> u32 {
    let (mut best, mut run) = (0, 0);
    for d in a..b {
        run = if ok(d) { run + 1 } else { 0 };
        best = best.max(run);
    }
    best
}

impl App {
    /// Library Stats › Reading: all time, or the day, week, month or year
    /// picked, stepped through with ‹ and ›.
    pub(crate) fn reading_section(&mut self, ui: &mut egui::Ui) {
        let (sum, set) = self.reading_view.summary();
        let first_day = sum.days.keys().next().copied().unwrap_or(sum.today);
        let view = &mut self.reading_view;
        let period = Period::of(view.unit, view.anchor.unwrap_or(sum.today));
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Reading").size(18.0).strong());
            ui.add_space(16.0);
            let before = view.unit;
            for (u, name) in [
                (Unit::All, "All time"),
                (Unit::Year, "Year"),
                (Unit::Month, "Month"),
                (Unit::Week, "Week"),
                (Unit::Day, "Day"),
            ] {
                ui.selectable_value(&mut view.unit, u, name);
            }
            if view.unit != before {
                // The new unit holding the day shown before (its first day).
                view.anchor = period.map(|p| p.start).filter(|_| view.unit != Unit::All);
                if period.is_some_and(|p| p.contains(sum.today)) {
                    view.anchor = None;
                }
            }
            let Some(p) = Period::of(view.unit, view.anchor.unwrap_or(sum.today)) else {
                return;
            };
            ui.separator();
            let name = view.unit.name();
            if ui
                .add_enabled(p.start > first_day, egui::Button::new("‹"))
                .on_hover_text(format!("The {name} before"))
                .clicked()
            {
                view.anchor = Some(p.step(-1).start);
            }
            ui.label(RichText::new(p.label()).strong());
            if ui
                .add_enabled(p.end <= sum.today, egui::Button::new("›"))
                .on_hover_text(format!("The {name} after"))
                .clicked()
            {
                let next = p.step(1);
                view.anchor = (!next.contains(sum.today)).then_some(next.start);
            }
            if !p.contains(sum.today) && ui.button(p.current_name()).clicked() {
                view.anchor = None;
            }
        });
        ui.add_space(4.0);
        let problem = rs::problem();
        if let Some(p) = &problem {
            ui.label(RichText::new(p).color(palette::warn()));
            ui.add_space(4.0);
        }
        if sum.days.is_empty() && sum.books.is_empty() {
            if problem.is_some() {
                return;
            }
            ui.label(
                RichText::new(
                    "Nothing timed yet. Open a book with 📕 Read: the reader times your reading and counts \
                     the Chinese characters you page past (P pauses it). Your time, speed and streaks show here.",
                )
                .color(palette::weak()),
            );
            return;
        }
        let before = period;
        let period = Period::of(
            self.reading_view.unit,
            self.reading_view.anchor.unwrap_or(sum.today),
        );
        // A new period brings the calendar to its year.
        if period != before {
            self.reading_view.year = None;
        }
        reading_tiles(ui, &sum, &set, period);
        self.reading_calendar(ui, &sum, period);
        ui.add_space(8.0);
        if period.is_none_or(|p| p.unit != Unit::Day) {
            if let Some(id) = self.reading_running(ui, &sum, &set, period) {
                self.open_book_id(&id, ui.ctx());
            }
            self.reading_amounts(ui, &sum, period);
            ui.add_space(8.0);
            reading_speeds(ui, &sum, period);
            ui.add_space(8.0);
        }
        self.reading_books(ui, &sum, &set, period);
        self.reading_remove_window(ui.ctx());
    }

    fn reading_calendar(&mut self, ui: &mut egui::Ui, sum: &Summary, period: Option<Period>) {
        let this_year = rs::civil(sum.today as i64).0;
        let first_year = sum
            .days
            .keys()
            .next()
            .map_or(this_year, |d| rs::civil(*d as i64).0)
            .min(this_year);
        // The period's year (where it starts) until ‹ › pick another.
        let year = self
            .reading_view
            .year
            .unwrap_or_else(|| period.map_or(this_year, |p| rs::civil(p.start as i64).0))
            .clamp(first_year, this_year);
        let first = rs::days_from_civil(year, 1, 1) as i32;
        let n = rs::days_from_civil(year + 1, 1, 1) as i32 - first;
        let months: Vec<(i32, &str)> = (1..=12)
            .map(|m| {
                (
                    rs::days_from_civil(year, m, 1) as i32,
                    MONTHS[m as usize - 1],
                )
            })
            .collect();
        let shades = calendar_shades();
        let year_secs = rs::sum_range(&sum.days, first, first + n).read_secs;
        let shown = |p: &Period| p.start < first + n && p.end > first;
        let sub = match period {
            Some(p) if p.unit != Unit::Year && shown(&p) => format!(
                "Minutes read each day in {year}: {}. {} stands out.",
                mins(year_secs),
                p.label()
            ),
            _ => format!("Minutes read each day in {year}: {}", mins(year_secs)),
        };
        let mut pick = None;
        card(ui, "Calendar", &sub, |ui| {
            {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(year > first_year, egui::Button::new("‹"))
                        .on_hover_text("The year before")
                        .clicked()
                    {
                        pick = Some(year - 1);
                    }
                    ui.label(RichText::new(year.to_string()).strong());
                    if ui
                        .add_enabled(year < this_year, egui::Button::new("›"))
                        .on_hover_text("The year after")
                        .clicked()
                    {
                        pick = Some(year + 1);
                    }
                });
                ui.add_space(4.0);
            }
            let day = |i: i32| -> Option<CalDay> {
                let d = first + i;
                let v = sum.days.get(&d)?;
                let level = day_level(v);
                let goal = sum.goal_days.contains(&d);
                Some(CalDay {
                    level,
                    ring: goal,
                    hover: day_hover(d, v, goal),
                })
            };
            charts::calendar(
                ui,
                charts::Calendar {
                    first,
                    weekday: rs::weekday(first),
                    n,
                    months: &months,
                    shades,
                    ring: palette::good(),
                    focus: period
                        .filter(|p| p.unit != Unit::Year && shown(p))
                        .map(|p| (p.start, p.end)),
                },
                &day,
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                let weak = palette::weak();
                ui.label(RichText::new("Less").small().color(weak));
                for s in shades {
                    let (r, _) =
                        ui.allocate_exact_size(egui::vec2(11.0, 11.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 2.0, s);
                }
                ui.label(RichText::new("More").small().color(weak))
                    .on_hover_text("Under 15 min, 15–30, 30–60, an hour or more");
                if !sum.goal_days.is_empty() || sum.goal.is_some() {
                    ui.add_space(10.0);
                    let (r, _) =
                        ui.allocate_exact_size(egui::vec2(11.0, 11.0), egui::Sense::hover());
                    ui.painter().rect_stroke(
                        r.shrink(0.5),
                        2.0,
                        egui::Stroke::new(1.5, palette::good()),
                        egui::StrokeKind::Inside,
                    );
                    ui.label(RichText::new("goal met").small().color(weak));
                }
            });
        });
        if let Some(y) = pick {
            self.reading_view.year = Some(y);
        }
    }

    /// Time or characters read: a day at a time (the last 30 or 90 days,
    /// a month or a week), or a month at a time over a year.
    fn reading_amounts(&mut self, ui: &mut egui::Ui, sum: &Summary, period: Option<Period>) {
        let v = &mut self.reading_view;
        let color = palette::cat().blue;
        // (days a..b, label, hover title).
        let spans: Vec<(i32, i32, String, String)> = match period {
            None => {
                let span = if v.long { 90 } else { 30 };
                (sum.today - span + 1..=sum.today)
                    .map(|d| {
                        let (_, m, dd) = rs::civil(d as i64);
                        (
                            d,
                            d + 1,
                            format!("{dd} {}", MONTHS[m as usize - 1]),
                            rs::long_date(d),
                        )
                    })
                    .collect()
            }
            Some(p) if p.unit == Unit::Year => {
                let (y, _, _) = rs::civil(p.start as i64);
                (1..=12)
                    .map(|m| {
                        (
                            month_start(y, m),
                            month_start(y, m + 1),
                            MONTHS[m as usize - 1].to_owned(),
                            format!("{} {y}", MONTH_NAMES[m as usize - 1]),
                        )
                    })
                    .collect()
            }
            Some(p) => (p.start..p.end)
                .map(|d| {
                    let (_, _, dd) = rs::civil(d as i64);
                    let label = if p.unit == Unit::Week {
                        format!("{} {dd}", &rs::long_date(d)[..3])
                    } else {
                        dd.to_string()
                    };
                    (d, d + 1, label, rs::long_date(d))
                })
                .collect(),
        };
        let bars: Vec<Bar> = spans
            .iter()
            .map(|(a, b, _, title)| {
                let day = rs::sum_range(&sum.days, *a, *b);
                let goal = b - a == 1 && sum.goal_days.contains(a);
                Bar {
                    value: match v.measure {
                        Measure::Time => day.read_secs / 60.0,
                        Measure::Chars => day.read_chars as f64,
                    },
                    color,
                    hover: span_hover(title.clone(), &day, goal),
                }
            })
            .collect();
        let labels: Vec<String> = spans.into_iter().map(|s| s.2).collect();
        let by_month = period.is_some_and(|p| p.unit == Unit::Year);
        let title = match (v.measure, by_month) {
            (Measure::Time, false) => "Minutes read a day",
            (Measure::Chars, false) => "Characters read a day",
            (Measure::Time, true) => "Minutes read a month",
            (Measure::Chars, true) => "Characters read a month",
        };
        let sub = match period {
            None => format!("The last {} days", if v.long { 90 } else { 30 }),
            Some(p) => p.label(),
        };
        card(ui, title, &sub, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut v.measure, Measure::Time, "Time");
                ui.selectable_value(&mut v.measure, Measure::Chars, "Characters");
                if period.is_none() {
                    ui.separator();
                    ui.selectable_value(&mut v.long, false, "30 days");
                    ui.selectable_value(&mut v.long, true, "90 days");
                }
            });
            charts::columns(ui, &bars, Some(&labels), CHART_HEIGHT, &charts::compact);
        });
    }

    fn reading_books(
        &mut self,
        ui: &mut egui::Ui,
        sum: &Summary,
        set: &rs::TrackerSettings,
        period: Option<Period>,
    ) {
        let mut remove = None;
        // Each book's figures for the period, and the books it has.
        let rows: Vec<(&String, &rs::BookRow, Day)> = sum
            .books
            .iter()
            .filter_map(|(id, b)| {
                let Some(p) = period else {
                    return Some((id, b, b.totals));
                };
                let t = rs::sum_range(&b.days, p.start, p.end);
                let finished = b.finished.is_some_and(|f| p.contains(rs::day_of(f, set)));
                (!t.is_empty() || finished).then_some((id, b, t))
            })
            .collect();
        let sub = match period {
            None => "Most recently read first. Right-click a title to remove its reading stats."
                .to_owned(),
            Some(p) => format!(
                "Time, characters and speed {} {}. Right-click a title to remove its reading stats.",
                p.prep(),
                p.label()
            ),
        };
        card(ui, "Books read in the reader", &sub, |ui| {
            if rows.is_empty() {
                ui.label(RichText::new("No book was read in this period.").color(palette::weak()));
                return;
            }
            egui::ScrollArea::horizontal()
                .id_salt("reading_books_scroll")
                .show(ui, |ui| {
                    egui::Grid::new("reading_books")
                        .striped(true)
                        .spacing([24.0, 6.0])
                        .show(ui, |ui| {
                            for (h, tip) in [
                                ("Book", ""),
                                ("Time", "Reading time, listening left out"),
                                ("Characters", "Chinese characters paged past"),
                                ("Speed", "Characters a minute of reading"),
                                ("Started", "The first day read"),
                                ("Finished", "Paged past its last page"),
                                (
                                    "Read",
                                    "Characters read of the book's Chinese characters, all time",
                                ),
                            ] {
                                let r = ui.strong(h);
                                if !tip.is_empty() {
                                    r.on_hover_text(tip);
                                }
                            }
                            ui.end_row();
                            let dash = || RichText::new("–").weak();
                            for (id, b, t) in &rows {
                                let title = if b.title.is_empty() {
                                    id.as_str()
                                } else {
                                    b.title.as_str()
                                };
                                let short: String = if title.chars().count() > 40 {
                                    title.chars().take(39).chain(['…']).collect()
                                } else {
                                    title.to_owned()
                                };
                                ui.label(short).on_hover_text(title).context_menu(|ui| {
                                    if ui.button("Remove reading stats…").clicked() {
                                        remove = Some(((*id).clone(), title.to_owned()));
                                        ui.close();
                                    }
                                });
                                if t.read_secs >= 1.0 {
                                    ui.label(mins(t.read_secs));
                                } else {
                                    ui.label(dash());
                                }
                                ui.label(fmt_int(t.read_chars));
                                match speed(t) {
                                    Some(v) => ui.label(format!("{v:.0}/min")),
                                    None => ui.label(dash()),
                                };
                                match b.first_read {
                                    Some(t) => ui.label(rs::date(rs::day_of(t, set))),
                                    None => ui.label(dash()),
                                };
                                match b.finished {
                                    Some(t) => ui.label(rs::date(rs::day_of(t, set))),
                                    None => ui.label(dash()),
                                };
                                if b.han_total > 0 {
                                    let p = (b.totals.read_chars as f64 * 100.0
                                        / b.han_total as f64)
                                        .min(100.0);
                                    ui.label(format!("{p:.0}%"));
                                } else {
                                    ui.label(dash());
                                }
                                ui.end_row();
                            }
                        });
                });
        });
        if remove.is_some() {
            self.reading_view.remove = remove;
        }
    }

    fn reading_remove_window(&mut self, ctx: &egui::Context) {
        let Some((id, title)) = self.reading_view.remove.clone() else {
            return;
        };
        let mut close = false;
        egui::Window::new("Remove reading stats")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(format!(
                    "Remove the time and characters read in “{title}”? This can't be undone."
                ));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Remove").clicked() {
                        rs::remove_book(&id);
                        if let Err(e) = rs::save() {
                            self.set_error("reading stats", e);
                        }
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if close {
            self.reading_view.remove = None;
        }
    }
}

fn goal_words(g: &rs::Goal) -> String {
    let mut what = Vec::new();
    if g.minutes > 0 {
        what.push(format!("{} min", g.minutes));
    }
    if g.chars > 0 {
        what.push(format!("{} chars", fmt_int(g.chars)));
    }
    format!("goal {} a day", what.join(" + "))
}

/// The tiles: all time, or the period's totals.
fn reading_tiles(
    ui: &mut egui::Ui,
    sum: &Summary,
    set: &rs::TrackerSettings,
    period: Option<Period>,
) {
    let read_day = |d: i32| {
        sum.days
            .get(&d)
            .is_some_and(|v| v.read_secs >= rs::DAY_READ_SECS)
    };
    let t = match period {
        None => sum.totals,
        Some(p) => rs::sum_range(&sum.days, p.start, p.end),
    };
    let days_read = match period {
        None => sum.days_read,
        Some(p) => (p.start..p.end).filter(|d| read_day(*d)).count(),
    };
    let time = if t.read_secs >= 1.0 {
        mins(t.read_secs)
    } else {
        "–".to_owned()
    };
    let mut tiles = vec![
        Tile::new(
            "Time read",
            RichText::new(time),
            match period {
                Some(p) if p.unit == Unit::Day => "listening left out".to_owned(),
                _ => format!("on {}", plural(days_read as u64, "day")),
            },
        )
        .hover("Time timed in the reader, idle stretches and listening left out"),
        Tile::new(
            "Characters read",
            RichText::new(fmt_int(t.read_chars)),
            "Chinese characters paged past".to_owned(),
        )
        .hover("Paging back takes characters off again; jumps (contents, links) don't count"),
    ];
    // Speed: a day needs a minute of reading, longer spans ten.
    let v = match period {
        Some(p) if p.unit == Unit::Day => speed(&t),
        _ => rs::speed_of(t.read_chars, t.read_secs),
    };
    let (value, sub) = match (v, period) {
        (Some(v), None) => (
            format!("{v:.0}/min"),
            match sum.speed {
                Some(m) => format!("all time · last 30 days {m:.0}/min"),
                None => "all time".to_owned(),
            },
        ),
        (Some(v), Some(p)) => (format!("{v:.0}/min"), p.label()),
        (None, _) => ("–".to_owned(), "not enough reading".to_owned()),
    };
    tiles.push(
        Tile::new("Speed", RichText::new(value), sub)
            .hover("Characters read ÷ minutes read, listening left out. Reading times use your speed over the last 30 days."),
    );
    match period {
        None => tiles.push(
            Tile::new(
                "Streak",
                RichText::new(plural(sum.read_streak.0, "day")),
                format!("longest {}", sum.read_streak.1),
            )
            .hover("Days in a row with a minute or more of reading. Today doesn't break a streak until it is over."),
        ),
        Some(p) if p.unit != Unit::Day => tiles.push(
            Tile::new(
                "Longest streak",
                RichText::new(plural(longest_run(p.start, p.end.min(sum.today + 1), read_day), "day")),
                format!("in a row, {}", p.label()),
            )
            .hover("Days in a row with a minute or more of reading"),
        ),
        _ => {}
    }
    if sum.goal.is_some() || !sum.goal_days.is_empty() {
        let words = sum
            .goal
            .as_ref()
            .map_or_else(|| "no goal now".to_owned(), goal_words);
        let met = |d: i32| sum.goal_days.contains(&d);
        tiles.push(match period {
            None => Tile::new(
                "Goal streak",
                RichText::new(plural(sum.goal_streak.0, "day")),
                format!("{words} · longest {}", sum.goal_streak.1),
            )
            .hover("Days in a row the daily goal was met (set in the reader's Stats panel)"),
            Some(p) if p.unit == Unit::Day => match rs::goal_in(&sum.goals, p.start) {
                Some(g) => Tile::new(
                    "Goal",
                    RichText::new(if met(p.start) { "Met" } else { "Not met" }),
                    goal_words(&g),
                ),
                None => Tile::new("Goal", RichText::new("–"), "no goal that day".to_owned()),
            },
            Some(p) => {
                // Only the days that had a goal, up to today.
                let upto = p.end.min(sum.today + 1);
                let had = |d: &i32| rs::goal_in(&sum.goals, *d).is_some();
                let n = (p.start..upto).filter(had).count() as u64;
                let hit = (p.start..upto).filter(|d| met(*d)).count() as u64;
                Tile::new(
                    "Goal met",
                    RichText::new(format!("{hit} of {}", plural(n, "day"))),
                    format!("longest run {}", longest_run(p.start, upto, met)),
                )
                .hover(
                    "Days the daily goal in force then was met (set in the reader's Stats panel)",
                )
            }
        });
    }
    let finished = match period {
        None => sum.books_finished,
        Some(p) => sum
            .books
            .iter()
            .filter(|(_, b)| b.finished.is_some_and(|f| p.contains(rs::day_of(f, set))))
            .count(),
    };
    tiles.push(Tile::new(
        "Books finished",
        RichText::new(fmt_int(finished as u64)),
        match period {
            None => format!("of {} read in the reader", sum.books.len()),
            Some(p) => p.label(),
        },
    ));
    if t.listen_secs >= 60.0 {
        tiles.push(
            Tile::new(
                "Listening",
                RichText::new(mins(t.listen_secs)),
                format!("{} characters", fmt_int(t.listen_chars)),
            )
            .hover("While read aloud played: kept apart from reading"),
        );
    }
    tile_grid(ui, tiles);
}

/// A book's title for a chart note: in 《》 when it is Chinese, cut short.
fn note_title(t: &str) -> String {
    const MAX: usize = 16;
    let mut s: String = t.trim().chars().take(MAX).collect();
    if t.trim().chars().count() > MAX {
        s.push('…');
    }
    if s.chars().any(hanstat::chars::is_han) {
        format!("《{s}》")
    } else {
        s
    }
}

/// Labels under a running total over days `first..end`: days, Mondays,
/// or month starts every 1–24 months, as many as `width` has room for.
fn running_ticks(first: i32, end: i32, width: f32) -> Vec<(i32, String)> {
    let span = end - first;
    let room = ((width / 80.0) as i32).max(2);
    let day_label = |d: i32| {
        let (_, m, dd) = rs::civil(d as i64);
        format!("{dd} {}", MONTHS[m as usize - 1])
    };
    if span <= room {
        return (first..end).map(|d| (d, day_label(d))).collect();
    }
    // Mondays, every week or every other one.
    for step in [7, 14] {
        if span <= step * room {
            let mut d = first + (7 - rs::weekday(first) as i32) % 7;
            let mut v = Vec::new();
            while d < end {
                v.push((d, day_label(d)));
                d += step;
            }
            return v;
        }
    }
    let (y0, m0, _) = rs::civil(first as i64);
    let (y1, _, _) = rs::civil(end as i64 - 1);
    // Month starts in range, on multiples of `step` counted from January
    // so labels land on the same months every year.
    let at_step = |step: i64| {
        let mut k = y0 * 12 + m0 as i64 - 1;
        if month_start(y0, m0 as i64) < first {
            k += 1;
        }
        while k % step != 0 {
            k += 1;
        }
        let mut v = Vec::new();
        loop {
            let (y, m) = (k.div_euclid(12), k.rem_euclid(12) + 1);
            let d = month_start(y, m);
            if d >= end {
                break v;
            }
            let name = MONTHS[m as usize - 1];
            let label = if y0 == y1 {
                name.to_owned()
            } else if step >= 12 {
                y.to_string()
            } else {
                format!("{name} {y}")
            };
            v.push((d, label));
            k += step;
        }
    };
    [1, 2, 3, 4, 6, 12, 24, 48]
        .into_iter()
        .map(at_step)
        .find(|v| v.len() as i32 <= room)
        .unwrap_or_default()
}

/// A book marked read with dates given by hand: its characters are spread
/// over days `from..=to`.
struct Dated {
    id: String,
    title: String,
    from: i32,
    to: i32,
    han: u64,
}

/// A book on the running total: characters by day, (timed by the reader,
/// spread over dates given by hand).
struct RunBook {
    id: String,
    title: String,
    days: BTreeMap<i32, (f64, f64)>,
    /// Finished in the reader (Unix time).
    finished: Option<u64>,
    by_hand: Option<(i32, i32)>,
}

/// The books read in the reader, and those marked read with dates: what
/// the reader didn't time of a dated book (its characters less those timed)
/// is spread evenly over its dates.
fn run_books(sum: &Summary, dated: &[Dated]) -> Vec<RunBook> {
    let mut out: Vec<RunBook> = sum
        .books
        .iter()
        .map(|(id, b)| RunBook {
            id: id.clone(),
            title: b.title.clone(),
            days: b
                .days
                .iter()
                .filter(|(_, d)| d.read_chars > 0)
                .map(|(k, d)| (*k, (d.read_chars as f64, 0.0)))
                .collect(),
            finished: b.finished,
            by_hand: None,
        })
        .collect();
    for m in dated {
        let i = match out.iter().position(|b| b.id == m.id) {
            Some(i) => i,
            None => {
                out.push(RunBook {
                    id: m.id.clone(),
                    title: m.title.clone(),
                    days: BTreeMap::new(),
                    finished: None,
                    by_hand: None,
                });
                out.len() - 1
            }
        };
        let b = &mut out[i];
        let timed: f64 = b.days.values().map(|d| d.0).sum();
        let left = (m.han as f64 - timed).max(0.0);
        let n = (m.to - m.from + 1).max(1);
        for d in m.from..m.from + n {
            b.days.entry(d).or_default().1 += left / n as f64;
        }
        b.by_hand = Some((m.from, m.to));
        if b.title.trim().is_empty() {
            b.title = m.title.clone();
        }
    }
    out
}

impl App {
    /// The Overview of Library book `id`, if its file is still there.
    fn open_book_id(&mut self, id: &str, ctx: &egui::Context) {
        if self.report.is_some() && self.report_id.as_deref() == Some(id) {
            self.show_history = false;
            self.home = false;
            return;
        }
        if let Some(e) = self.history.entries.iter().find(|e| e.id == id)
            && e.path.exists()
        {
            self.open(e.path.clone(), ctx);
        }
    }

    /// The path of Library book `id`, if its file is still there.
    fn book_path(&self, id: &str) -> Option<&std::path::Path> {
        self.history
            .entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.path.as_path())
            .filter(|p| p.exists())
    }

    /// Books marked read with dates given by hand, for [`run_books`].
    fn dated_marks(&self, day_start_hour: u32) -> Vec<Dated> {
        self.known
            .settings
            .read
            .iter()
            .filter(|m| m.started.is_some() || m.finished.is_some())
            .filter_map(|m| {
                let han = if m.han > 0 {
                    m.han
                } else {
                    let e = self.history.entries.iter().find(|e| e.id == m.id)?;
                    e.stats.han_total
                };
                let marked = rs::local_day(m.read as f64, day_start_hour);
                let a = m.started.or(m.finished)?;
                let b = m.finished.unwrap_or(marked).max(a);
                Some(Dated {
                    id: m.id.clone(),
                    title: m.title.clone(),
                    from: a,
                    to: b,
                    han,
                })
            })
            .collect()
    }

    /// Characters read, added up day by day, with each book noted where half
    /// of what was read of it (in the period) had been read. Books marked
    /// read with dates add their characters spread over those days. Returns
    /// the id of the book whose note was clicked.
    fn reading_running(
        &self,
        ui: &mut egui::Ui,
        sum: &Summary,
        set: &rs::TrackerSettings,
        period: Option<Period>,
    ) -> Option<String> {
        let all = run_books(sum, &self.dated_marks(set.day_start_hour));
        let first_read = all
            .iter()
            .filter_map(|b| b.days.keys().next())
            .min()
            .copied();
        let (first, end) = match period {
            // A week at least, so a first day or two isn't one steep line.
            None => (
                first_read.unwrap_or(sum.today).min(sum.today - 6),
                sum.today + 1,
            ),
            Some(p) => (p.start, p.end.min(sum.today + 1).max(p.start + 1)),
        };
        let mut by_day: BTreeMap<i32, f64> = BTreeMap::new();
        for b in &all {
            for (d, (r, h)) in b.days.range(first..end) {
                *by_day.entry(*d).or_default() += r + h;
            }
        }
        let steps: Vec<(i32, f64)> = by_day.into_iter().filter(|s| s.1 > 0.0).collect();
        let total = steps.iter().map(|s| s.1).sum::<f64>().round() as u64;
        let sub = match period {
            None => match first_read {
                Some(d) => format!(
                    "{} characters since {}, added up day by day",
                    fmt_int(total),
                    rs::long_date(d)
                ),
                None => String::new(),
            },
            Some(p) => format!(
                "{} characters {} {}, added up day by day",
                fmt_int(total),
                p.prep(),
                p.label()
            ),
        };
        // Each book: (characters read in range, the day half of them were
        // read, first and last day read in range, the book).
        let mut books: Vec<(f64, i32, i32, i32, &RunBook)> = all
            .iter()
            .filter_map(|b| {
                let days: Vec<(i32, f64)> = b
                    .days
                    .range(first..end)
                    .map(|(k, (r, h))| (*k, r + h))
                    .filter(|d| d.1 > 0.0)
                    .collect();
                let n: f64 = days.iter().map(|d| d.1).sum();
                if n < 0.5 {
                    return None;
                }
                let mut acc = 0.0;
                let half = days
                    .iter()
                    .find(|(_, c)| {
                        acc += c;
                        acc * 2.0 >= n
                    })
                    .map_or(days[0].0, |d| d.0);
                Some((n, half, days[0].0, days[days.len() - 1].0, b))
            })
            .collect();
        books.sort_by(|a, b| b.0.total_cmp(&a.0));
        let notes: Vec<charts::Note> = books
            .iter()
            .map(|&(n, half, a, z, b)| {
                let when = if a == z {
                    format!("on {}", rs::long_date(a))
                } else {
                    format!("{} – {}", rs::long_date(a), rs::long_date(z))
                };
                let mut hover = format!(
                    "{}\n{} characters read {when}",
                    b.title.trim(),
                    fmt_int(n.round() as u64)
                );
                if let Some((from, to)) = b.by_hand {
                    hover.push_str(&format!(
                        "\nRead {} – {} (dates given in Mark read): its characters are spread evenly over those days{}",
                        rs::date(from),
                        rs::date(to),
                        if b.days.values().any(|d| d.0 > 0.0) {
                            ", less what the reader timed"
                        } else {
                            ""
                        }
                    ));
                }
                if let Some(f) = b.finished {
                    hover.push_str(&format!(
                        "\nFinished in the reader {}",
                        rs::long_date(rs::local_day(f as f64, 0))
                    ));
                }
                let link = self.book_path(&b.id).is_some();
                if link {
                    hover.push_str("\nClick to open it");
                }
                charts::Note {
                    x: half as f64 + 0.5,
                    text: note_title(&b.title),
                    hover,
                    link,
                    span: Some((a as f64, z as f64 + 1.0)),
                }
            })
            .collect();
        let mut clicked = None;
        card(ui, "Characters read over time", &sub, |ui| {
            if total == 0 {
                ui.label(
                    RichText::new(match period {
                        Some(p) => format!("No characters read {} {}.", p.prep(), p.label()),
                        None => "No characters read yet.".to_owned(),
                    })
                    .color(palette::weak()),
                );
                return;
            }
            let ticks = running_ticks(first, end, ui.available_width() - 62.0);
            // Running totals by day, for the hover.
            let mut running = Vec::with_capacity(steps.len());
            let mut acc = 0.0;
            for (d, v) in &steps {
                acc += v;
                running.push((*d, acc));
            }
            let hover = |day: i32, _: f64| {
                let i = running.partition_point(|r| r.0 <= day);
                let so_far = if i == 0 { 0.0 } else { running[i - 1].1 };
                let mut t = format!(
                    "{}\n{} characters so far",
                    rs::long_date(day),
                    fmt_int(so_far.round() as u64)
                );
                let mut any = false;
                for (.., b) in &books {
                    let Some((r, h)) = b.days.get(&day) else {
                        continue;
                    };
                    if r + h < 0.5 {
                        continue;
                    }
                    any = true;
                    t.push_str(&format!(
                        "\n+{} {}{}",
                        fmt_int((r + h).round() as u64),
                        b.title.trim(),
                        if *h > 0.0 {
                            " (spread over its dates)"
                        } else {
                            ""
                        }
                    ));
                }
                if !any {
                    t.push_str("\nNothing read that day");
                }
                t
            };
            clicked = charts::running_total(
                ui,
                charts::Running {
                    first,
                    end,
                    steps: &steps,
                    notes: &notes,
                    ticks: &ticks,
                    color: palette::cat().mauve,
                    height: 320.0,
                },
                &hover,
            );
        });
        clicked.map(|i| books[i].4.id.clone())
    }
}

/// Reading speed by week (the last half year, or a year's weeks) or by
/// day (a month or a week).
fn reading_speeds(ui: &mut egui::Ui, sum: &Summary, period: Option<Period>) {
    const WEEKS: i32 = 26;
    let week_label = |w: i32| {
        let (_, m, d) = rs::civil(w as i64);
        format!("{d} {}", MONTHS[m as usize - 1])
    };
    // (days a..b, label, hover title); weeks or days.
    let (spans, by_day): (Vec<(i32, i32, String, String)>, bool) = match period {
        None => {
            let this_week = sum.today - rs::weekday(sum.today) as i32;
            (
                (0..WEEKS)
                    .rev()
                    .map(|k| {
                        let w = this_week - 7 * k;
                        (
                            w,
                            w + 7,
                            week_label(w),
                            format!("Week of {}", rs::long_date(w)),
                        )
                    })
                    .collect(),
                false,
            )
        }
        Some(p) if p.unit == Unit::Year => {
            let mut v = Vec::new();
            let mut w = p.start - rs::weekday(p.start) as i32;
            while w < p.end {
                v.push((
                    w,
                    w + 7,
                    week_label(w),
                    format!("Week of {}", rs::long_date(w)),
                ));
                w += 7;
            }
            (v, false)
        }
        Some(p) => (
            (p.start..p.end)
                .map(|d| {
                    let (_, _, dd) = rs::civil(d as i64);
                    let label = if p.unit == Unit::Week {
                        format!("{} {dd}", &rs::long_date(d)[..3])
                    } else {
                        dd.to_string()
                    };
                    (d, d + 1, label, rs::long_date(d))
                })
                .collect(),
            true,
        ),
    };
    // A day needs 3 minutes of reading for a speed, a week 10.
    let min_secs = if by_day { 180.0 } else { rs::MIN_SPEED_SECS };
    let color = palette::cat().teal;
    let mut any = false;
    let bars: Vec<Bar> = spans
        .iter()
        .map(|(a, b, _, title)| {
            let d = rs::sum_range(&sum.days, *a, *b);
            let v = (d.read_secs >= min_secs && d.read_chars > 0)
                .then(|| d.read_chars as f64 / (d.read_secs / 60.0));
            any |= v.is_some();
            Bar {
                value: v.unwrap_or(0.0),
                color,
                hover: match v {
                    Some(v) => format!(
                        "{title}\n{v:.0} chars/min: {} characters in {}",
                        fmt_int(d.read_chars),
                        mins(d.read_secs)
                    ),
                    None => format!(
                        "{title}\nUnder {} min of reading: no speed",
                        min_secs as u32 / 60
                    ),
                },
            }
        })
        .collect();
    let labels: Vec<String> = spans.into_iter().map(|s| s.2).collect();
    let (title, unit) = if by_day {
        ("Reading speed by day", "days")
    } else {
        ("Reading speed by week", "weeks")
    };
    let mut sub = format!(
        "Characters a minute, {unit} with {} min of reading or more",
        min_secs as u32 / 60
    );
    if let Some(p) = period {
        sub.push_str(&format!(" · {}", p.label()));
    }
    card(ui, title, &sub, |ui| {
        if any {
            charts::columns(ui, &bars, Some(&labels), CHART_HEIGHT, &|v| {
                format!("{v:.0}")
            });
        } else {
            ui.label(
                RichText::new(format!(
                    "None of these {unit} has {} minutes of reading.",
                    min_secs as u32 / 60
                ))
                .color(palette::weak()),
            );
        }
    });
}

/// "Read so far" rows for book `id` in its Overview's grid; nothing if it
/// was never read in the reader.
/// `cpm`, `by_hand`: the speed reading times use, and whether it was set by
/// hand (in Settings, or for this book on its Overview).
pub(crate) fn book_rows(ui: &mut egui::Ui, id: &str, han_total: u64, cpm: f32, by_hand: bool) {
    let now = rs::now();
    let Some((t, first, finished, reader_han, book_speed, overall, set)) = rs::with(|s| {
        let b = s.books.get(id)?;
        let today = rs::local_day(now, s.settings.day_start_hour);
        Some((
            b.totals(),
            b.first_read,
            b.finished,
            b.han_total,
            b.speed(),
            s.measured_speed(today),
            s.settings.clone(),
        ))
    }) else {
        return;
    };
    if t.is_empty() && finished.is_none() {
        return;
    }
    let total = if reader_han > 0 {
        reader_han
    } else {
        han_total
    };
    ui.label("Read so far");
    let mut s = format!(
        "{}, {} characters",
        mins(t.read_secs),
        fmt_int(t.read_chars)
    );
    if total > 0 {
        s.push_str(&format!(
            " ({:.0}%)",
            (t.read_chars as f64 * 100.0 / total as f64).min(100.0)
        ));
    }
    if let Some(v) = speed(&t) {
        s.push_str(&format!(", {v:.0} chars/min"));
    }
    ui.label(s)
        .on_hover_text("Timed in the reader: paging back takes characters off, jumps don't count, listening is left out");
    ui.end_row();
    let mut dates = Vec::new();
    if let Some(f) = first {
        dates.push(format!("started {}", rs::date(rs::day_of(f, &set))));
    }
    if let Some(f) = finished {
        dates.push(format!("finished {}", rs::date(rs::day_of(f, &set))));
    }
    if !dates.is_empty() {
        ui.label("In the reader");
        let mut d = dates.join(", ");
        d[..1].make_ascii_uppercase();
        ui.label(d);
        ui.end_row();
    }
    if finished.is_none() && total > t.read_chars {
        let (v, source) = match (by_hand, book_speed, overall) {
            (true, _, _) => (cpm as f64, "the speed set by hand"),
            (false, Some(v), _) => (v, "your speed in this book"),
            (false, None, Some(v)) => (v, "your speed over the last 30 days"),
            (false, None, None) => (cpm as f64, "the speed in Settings, until yours is measured"),
        };
        ui.label("Left");
        ui.label(format!(
            "≈ {} at {v:.0} chars/min",
            mins((total - t.read_chars) as f64 / v.max(1.0) * 60.0)
        ))
        .on_hover_text(format!("The characters not read yet, at {source}"));
        ui.end_row();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i64, m: i64, d: i64) -> i32 {
        rs::days_from_civil(y, m, d) as i32
    }

    #[test]
    fn periods_hold_their_day_and_step_by_one() {
        let mon = day(2026, 11, 23); // a Monday
        let p = |u| Period::of(u, day(2026, 11, 26)).unwrap();
        assert_eq!(Period::of(Unit::All, mon), None);
        assert_eq!(
            (p(Unit::Day).start, p(Unit::Day).end),
            (day(2026, 11, 26), day(2026, 11, 27))
        );
        assert_eq!((p(Unit::Week).start, p(Unit::Week).end), (mon, mon + 7));
        assert_eq!(
            (p(Unit::Month).start, p(Unit::Month).end),
            (day(2026, 11, 1), day(2026, 12, 1))
        );
        assert_eq!(
            (p(Unit::Year).start, p(Unit::Year).end),
            (day(2026, 1, 1), day(2027, 1, 1))
        );
        // Across the year's end, both ways.
        let dec = Period::of(Unit::Month, day(2026, 12, 31)).unwrap();
        assert_eq!(dec.step(1).start, day(2027, 1, 1));
        assert_eq!(dec.step(1).step(-1), dec);
        assert_eq!(
            Period::of(Unit::Month, day(2026, 1, 5))
                .unwrap()
                .step(-1)
                .start,
            day(2025, 12, 1)
        );
        assert_eq!(p(Unit::Year).step(-1).start, day(2025, 1, 1));
        assert_eq!(p(Unit::Week).step(-1).start, mon - 7);
        assert_eq!(p(Unit::Day).step(1).start, day(2026, 11, 27));
        // February in a leap year.
        let feb = Period::of(Unit::Month, day(2028, 2, 10)).unwrap();
        assert_eq!(feb.end - feb.start, 29);
    }

    #[test]
    fn period_labels() {
        assert_eq!(
            Period::of(Unit::Day, day(2026, 11, 23)).unwrap().label(),
            "Mon 23 Nov 2026"
        );
        assert_eq!(
            Period::of(Unit::Week, day(2026, 11, 26)).unwrap().label(),
            "23–29 Nov 2026"
        );
        assert_eq!(
            Period::of(Unit::Week, day(2026, 12, 2)).unwrap().label(),
            "30 Nov – 6 Dec 2026"
        );
        assert_eq!(
            Period::of(Unit::Week, day(2026, 12, 30)).unwrap().label(),
            "28 Dec 2026 – 3 Jan 2027"
        );
        assert_eq!(
            Period::of(Unit::Month, day(2026, 11, 5)).unwrap().label(),
            "November 2026"
        );
        assert_eq!(
            Period::of(Unit::Year, day(2026, 11, 5)).unwrap().label(),
            "2026"
        );
    }

    #[test]
    fn longest_runs() {
        let ok = |d: i32| ![3, 7].contains(&d);
        assert_eq!(longest_run(0, 10, ok), 3);
        assert_eq!(longest_run(5, 5, ok), 0);
    }

    #[test]
    fn dated_books_spread_what_the_reader_did_not_time() {
        let mut sum = Summary::default();
        let mut row = rs::BookRow {
            title: "长风渡".into(),
            ..Default::default()
        };
        row.days.insert(
            10,
            Day {
                read_chars: 1000,
                ..Default::default()
            },
        );
        sum.books.push(("a".into(), row));
        let dated = |id: &str, from, to, han| Dated {
            id: id.into(),
            title: "书".into(),
            from,
            to,
            han,
        };
        let b = run_books(&sum, &[dated("a", 10, 13, 5000), dated("b", 20, 20, 300)]);
        assert_eq!(b.len(), 2);
        // 5,000 less the 1,000 timed, over four days.
        assert_eq!(b[0].days[&10], (1000.0, 1000.0));
        assert_eq!(b[0].days[&13], (0.0, 1000.0));
        assert_eq!(b[0].by_hand, Some((10, 13)));
        // Not read in the reader: all on its one day.
        assert_eq!(b[1].days[&20], (0.0, 300.0));
        // Timed past its length: nothing more is added.
        let b = run_books(&sum, &[dated("a", 10, 11, 800)]);
        assert_eq!(b[0].days[&11], (0.0, 0.0));
    }

    #[test]
    fn running_ticks_pick_days_weeks_or_months() {
        let labels = |v: Vec<(i32, String)>| v.into_iter().map(|t| t.1).collect::<Vec<_>>();
        // A week, room for every day.
        let mon = day(2026, 11, 23);
        assert_eq!(
            labels(running_ticks(mon, mon + 3, 800.0)),
            ["23 Nov", "24 Nov", "25 Nov"]
        );
        // A month: Mondays.
        let nov = day(2026, 11, 1);
        assert_eq!(
            labels(running_ticks(nov, day(2026, 12, 1), 400.0)),
            ["2 Nov", "9 Nov", "16 Nov", "23 Nov", "30 Nov"]
        );
        // Two months in 400 px: every other Monday.
        let t = running_ticks(nov, nov + 60, 400.0);
        assert_eq!(t.len(), 5, "{t:?}");
        assert_eq!(t[1].0 - t[0].0, 14);
        // A year: month names alone.
        let t = running_ticks(day(2026, 1, 1), day(2027, 1, 1), 1000.0);
        assert_eq!(t[0], (day(2026, 1, 1), "Jan".to_owned()));
        assert_eq!(t.len(), 12);
        // Four years from mid-March: months with years, from April, every
        // few months counted from January.
        let t = running_ticks(day(2023, 3, 15), day(2027, 3, 1), 900.0);
        assert!(t.len() <= 11, "{t:?}");
        assert!(t[0].0 >= day(2023, 3, 15));
        assert!(
            t.iter().all(|(_, s)| s.ends_with(char::is_numeric)),
            "{t:?}"
        );
        assert_eq!(t[0].1, "Jul 2023");
        assert_eq!(note_title("死亡万花筒"), "《死亡万花筒》");
        assert_eq!(note_title("Journey to the West"), "Journey to the W…");
    }
}
