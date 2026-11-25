//! Home: the start page. The book being read, large, with the way back into
//! it; the other books read lately; today's reading against the goal; this
//! month's reading as a calendar; and the totals. With an empty Library, a
//! welcome page with a place to drop books instead.
//!
//! Its figures are the reader's ([`crate::reading_stats`]). How far through
//! each book the reader is (from the chapter lengths saved by
//! [`crate::chapter_lengths`]), and the large covers, are worked out on a
//! background thread the first time Home shows a book.

use crate::library_reading::{calendar_shades, day_hover, day_level};
use crate::reader::{self, stats_mins as mins};
use crate::reading_stats::{self as rs, LeftAt, Summary};
use crate::{App, HistTab, charts, drop_works, fmt_int, palette};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense,
    Shadow, Stroke, StrokeKind, TextureHandle, Ui, UiBuilder, Vec2, pos2, vec2,
};
use hanstat::extract::{self, EncodingChoice};
use hanstat::history;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// The page's widest column (points): wider windows get margins.
const MAX_W: f32 = 1120.0;
/// Below this column width, side-by-side parts stack.
const WIDE: f32 = 900.0;
/// Space between the page's sections.
const SECTION: f32 = 40.0;
/// Covers keep a paperback's proportions (about 1 : 1.42) and are cut to fit.
const HERO_COVER: Vec2 = vec2(184.0, 262.0);
const SHELF_COVER: Vec2 = vec2(120.0, 171.0);
const SHELF_GAP: f32 = 24.0;
const SHELF_MAX: usize = 8;
/// The Today column beside the hero.
const TODAY_W: f32 = 250.0;
const RING: f32 = 104.0;
/// The year calendar's width: cells of about 11 points.
const YEAR_W: f32 = 30.0 + 53.0 * 14.0;

const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const MONTHS: [&str; 12] = [
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

/// What Home keeps between frames.
#[derive(Default)]
pub(crate) struct HomeView {
    /// Made again only when the stats or the day change.
    summary: Option<(u64, i32, Arc<Summary>, rs::TrackerSettings)>,
    /// By book id; `None` while its details are being worked out.
    books: HashMap<String, Option<BookInfo>>,
    rx: Option<Receiver<Found>>,
    /// Known words and characters, counted for the knowledge at this address.
    known: Option<(usize, u64, u64)>,
    /// The daily goal being edited (minutes, characters).
    goal_edit: Option<(u32, u64)>,
}

/// A book's details for Home.
struct BookInfo {
    /// Characters in each chapter, as the reader counts them (its places
    /// are offsets in these); `None`: not an EPUB the reader can open.
    chapters: Option<Arc<[usize]>>,
    /// The table of contents: spine index and label, in its order.
    toc: Vec<(usize, String)>,
    /// The large cover; `None`: there is none (the small one stands in).
    cover: Option<TextureHandle>,
}

/// A book's details, from the background thread.
struct Found {
    id: String,
    chapters: Option<Arc<[usize]>>,
    toc: Vec<(usize, String)>,
    cover: Option<egui::ColorImage>,
}

/// A book to ask the background thread about.
struct Ask {
    id: String,
    path: PathBuf,
    epub: bool,
}

/// A book shown on Home.
struct Shown {
    id: String,
    title: String,
    path: PathBuf,
    epub: bool,
    traditional: bool,
    han_total: u64,
    /// Where the reader left it; `None`: never opened in the reader.
    at: Option<LeftAt>,
    finished: bool,
}

/// What a click on Home asks for, done once the page is drawn.
enum Act {
    Read(usize),
    Overview(usize),
    Library(HistTab),
    Open,
    Anki,
    Known,
    Goal(u32, u64),
}

impl HomeView {
    fn summary(&mut self) -> (Arc<Summary>, rs::TrackerSettings) {
        let now = rs::now();
        let rev = rs::revision();
        let today = rs::with(|s| rs::local_day(now, s.settings.day_start_hour));
        if let Some((r, t, s, set)) = &self.summary
            && *r == rev
            && *t == today
        {
            return (s.clone(), set.clone());
        }
        let (sum, set) = rs::with(|s| (Arc::new(s.summary(today)), s.settings.clone()));
        self.summary = Some((rev, today, sum.clone(), set.clone()));
        (sum, set)
    }

    /// Take in what the background thread found.
    fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else { return };
        loop {
            match rx.try_recv() {
                Ok(f) => {
                    let cover = f.cover.map(|img| {
                        ctx.load_texture(
                            format!("home cover {}", f.id),
                            img,
                            egui::TextureOptions {
                                mipmap_mode: Some(egui::TextureFilter::Linear),
                                ..egui::TextureOptions::LINEAR
                            },
                        )
                    });
                    self.books.insert(
                        f.id,
                        Some(BookInfo {
                            chapters: f.chapters,
                            toc: f.toc,
                            cover,
                        }),
                    );
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    // Books still waiting were not found: asked again later.
                    self.books.retain(|_, b| b.is_some());
                    break;
                }
            }
        }
    }

    /// Work out the details of the books in `ask` not known yet, in order,
    /// one thread at a time.
    fn request(&mut self, ask: Vec<Ask>, ctx: &egui::Context) {
        if self.rx.is_some() {
            return;
        }
        let ask: Vec<Ask> = ask
            .into_iter()
            .filter(|a| !self.books.contains_key(&a.id))
            .collect();
        if ask.is_empty() {
            return;
        }
        for a in &ask {
            self.books.insert(a.id.clone(), None);
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for a in ask {
                if tx.send(find(a)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        self.rx = Some(rx);
    }

    /// Forget a book's details (its file was replaced or it was removed).
    pub(crate) fn forget(&mut self, id: &str) {
        self.books.remove(id);
    }
}

/// A book's chapter lengths and contents, and its large cover (made from
/// the book the first time: books analysed before Home have only the small
/// one).
fn find(a: Ask) -> Found {
    let (mut chapters, mut toc) = (None, Vec::new());
    if a.epub
        && let Ok(epub) = reader::epub::Epub::open(&a.path)
    {
        chapters = Some(crate::chapter_lengths::of_open(&a.id, &epub));
        toc = epub
            .toc
            .iter()
            .map(|t| (t.spine, t.label.trim().to_owned()))
            .filter(|(_, l)| !l.is_empty())
            .collect();
    }
    let large = history::large_cover_path(&a.id);
    // Only books with a small cover have one to find.
    if a.epub
        && !large.is_file()
        && history::cover_path(&a.id).is_file()
        && let Ok(book) = extract::load(&a.path, EncodingChoice::Auto)
        && let Some(c) = &book.cover
    {
        let _ = history::save_large_cover(c, &large);
    }
    let cover = image::open(&large).ok().map(|img| {
        let img = img.into_rgba8();
        egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            img.as_raw(),
        )
    });
    Found {
        id: a.id,
        chapters,
        toc,
        cover,
    }
}

/// The contents entry the chapter at `spine` comes under: the last entry
/// at or before it.
fn chapter_name(toc: &[(usize, String)], spine: usize) -> Option<&str> {
    let best = toc.iter().map(|t| t.0).filter(|&s| s <= spine).max()?;
    toc.iter().find(|t| t.0 == best).map(|t| t.1.as_str())
}

/// "about 4 h 10 min left" for the rest of a book of `han` characters at
/// `cpm` characters a minute; `None` when there is nothing to go on.
fn time_left(han: u64, done: f32, cpm: f32) -> Option<String> {
    if han == 0 || cpm <= 0.0 {
        return None;
    }
    let m = (han as f64 * (1.0 - done as f64).max(0.0) / cpm as f64).round() as u64;
    Some(if m < 1 {
        "less than a minute left".to_owned()
    } else if m < 60 {
        format!("about {m} min left")
    } else {
        // To five minutes: the speed is no closer than that.
        let m = (m + 2) / 5 * 5;
        match m % 60 {
            0 => format!("about {} h left", m / 60),
            r => format!("about {} h {r} min left", m / 60),
        }
    })
}

/// "42%", rounded down, and "<1%" for a start.
fn percent(p: f32) -> String {
    let n = (p * 100.0).floor();
    if p > 0.0 && n < 1.0 {
        "<1%".to_owned()
    } else {
        format!("{n:.0}%")
    }
}

/// "today", "yesterday", "3 days ago".
fn ago(day: i32, today: i32) -> String {
    match today - day {
        ..=0 => "today".to_owned(),
        1 => "yesterday".to_owned(),
        n => format!("{n} days ago"),
    }
}

/// Short [`ago`], for under the shelf's covers.
fn ago_short(day: i32, today: i32) -> String {
    match today - day {
        ..=0 => "today".to_owned(),
        1 => "yesterday".to_owned(),
        n if n < 7 => format!("{n} d ago"),
        n if n < 60 => format!("{} wk ago", n / 7),
        n => format!("{} mo ago", n / 30),
    }
}

/// "Good morning" and the like, by the hour.
fn greeting(hour: i64) -> &'static str {
    match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    }
}

/// How many shelf covers fit in `w`.
fn shelf_fits(w: f32) -> usize {
    (((w + SHELF_GAP) / (SHELF_COVER.x + SHELF_GAP)).floor() as usize).max(1)
}

/// The accent a coverless book's tile takes: the same for the same book.
fn tile_accent(id: &str) -> Color32 {
    let c = palette::cat();
    let accents = [
        c.mauve, c.blue, c.teal, c.peach, c.pink, c.sapphire, c.green, c.lavender, c.flamingo,
    ];
    let h = id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    });
    accents[(h % accents.len() as u64) as usize]
}

/// The part of a `size` image that fills a box of `aspect` (width ÷
/// height), centred: the rest is cut off.
fn cover_uv(size: Vec2, aspect: f32) -> Rect {
    let a = size.x / size.y.max(1.0);
    if a > aspect {
        let k = (1.0 - aspect / a) / 2.0;
        Rect::from_min_max(pos2(k, 0.0), pos2(1.0 - k, 1.0))
    } else {
        let k = (1.0 - a / aspect) / 2.0;
        Rect::from_min_max(pos2(0.0, k), pos2(1.0, 1.0 - k))
    }
}

/// A cover in `rect`, with a soft shadow that grows with `lift` (0..=1);
/// a tile with the title when there is no cover.
fn paint_cover(ui: &Ui, rect: Rect, tex: Option<&TextureHandle>, title: &str, id: &str, lift: f32) {
    let p = ui.painter();
    let dark = palette::dark();
    let radius = CornerRadius::same(8);
    let shadow = Shadow {
        offset: [0, (4.0 + 6.0 * lift) as i8],
        blur: (14.0 + 12.0 * lift) as u8,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 110 } else { 38 }),
    };
    p.add(shadow.as_shape(rect, radius));
    match tex {
        Some(t) => {
            let uv = cover_uv(t.size_vec2(), rect.width() / rect.height());
            egui::Image::from_texture(egui::load::SizedTexture::from_handle(t))
                .uv(uv)
                .corner_radius(radius)
                .paint_at(ui, rect);
        }
        None => {
            let c = palette::cat();
            let accent = tile_accent(id);
            p.rect_filled(rect, radius, palette::mix(accent, c.base, 0.28));
            let band = Rect::from_min_size(rect.min, vec2(rect.width(), 6.0));
            p.rect_filled(
                band,
                CornerRadius {
                    nw: 8,
                    ne: 8,
                    sw: 0,
                    se: 0,
                },
                accent,
            );
            let size = if rect.width() > 150.0 { 17.0 } else { 13.0 };
            let mut job = egui::text::LayoutJob::simple(
                title.to_owned(),
                palette::bold(size),
                c.text,
                rect.width() - 24.0,
            );
            job.wrap.max_rows = 6;
            job.wrap.break_anywhere = true;
            job.halign = Align::Center;
            let g = p.layout_job(job);
            p.galley(
                pos2(rect.center().x, rect.min.y + rect.height() * 0.3),
                g,
                c.text,
            );
        }
    }
    p.rect_stroke(
        rect,
        radius,
        Stroke::new(
            1.0,
            if dark {
                Color32::from_white_alpha(14)
            } else {
                Color32::from_black_alpha(22)
            },
        ),
        StrokeKind::Inside,
    );
}

/// A thin rounded bar `done` (0..=1) of the way.
pub(crate) fn progress_bar(ui: &Ui, rect: Rect, done: f32) {
    let c = palette::cat();
    let r = rect.height() / 2.0;
    ui.painter().rect_filled(rect, r, c.surface0);
    if done > 0.0 {
        let w = (rect.width() * done.clamp(0.0, 1.0)).max(rect.height());
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, vec2(w, rect.height())),
            r,
            c.mauve,
        );
    }
}

/// The ring's colour at `done` (0..=1): red at the start, through orange
/// (peach) and yellow, to green when the goal is met. Yellow sits between
/// peach and green because blending those two directly goes through khaki.
fn ring_colour(c: &palette::Flavour, done: f32) -> Color32 {
    let stops = [
        (0.0, c.red),
        (0.45, c.peach),
        (0.75, c.yellow),
        (1.0, c.green),
    ];
    let done = done.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        let ((a, from), (b, to)) = (w[0], w[1]);
        if done <= b {
            return palette::mix(to, from, (done - a) / (b - a));
        }
    }
    c.green
}

/// A ring of `RING` points, `done` (0..=1) of it drawn round from the top.
fn ring(ui: &mut Ui, done: f32, middle: &str, under: &str, hover: &str) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(RING), Sense::hover());
    resp.on_hover_text(hover);
    let p = ui.painter();
    let c = palette::cat();
    let width = 9.0;
    let r = RING / 2.0 - width / 2.0 - 1.0;
    let centre = rect.center();
    p.circle_stroke(centre, r, Stroke::new(width, c.surface0));
    let done = done.clamp(0.0, 1.0);
    if done > 0.0 {
        let n = (96.0 * done).ceil().max(2.0) as usize;
        let at = |i: usize| {
            let a =
                -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * done * i as f32 / n as f32;
            centre + r * vec2(a.cos(), a.sin())
        };
        let points: Vec<Pos2> = (0..=n).map(at).collect();
        let fill = ring_colour(c, done);
        // Round ends.
        p.circle_filled(points[0], width / 2.0, fill);
        p.circle_filled(points[n], width / 2.0, fill);
        p.add(egui::Shape::line(points, Stroke::new(width, fill)));
    }
    p.text(
        centre - vec2(0.0, 6.0),
        Align2::CENTER_CENTER,
        middle,
        palette::bold(24.0),
        c.text,
    );
    p.text(
        centre + vec2(0.0, 15.0),
        Align2::CENTER_CENTER,
        under,
        FontId::proportional(11.0),
        palette::weak(),
    );
}

/// A section's title, its count, and a link at the right.
fn section_head(ui: &mut Ui, title: &str, count: Option<usize>, link: Option<&str>) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(palette::bold_text(title, 19.0).color(palette::strong()));
        if let Some(n) = count {
            ui.label(
                RichText::new(n.to_string())
                    .size(15.0)
                    .color(palette::weak()),
            );
        }
        if let Some(l) = link {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                clicked = ui
                    .add(egui::Button::new(RichText::new(l).color(palette::weak())).frame(false))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked();
            });
        }
    });
    clicked
}

/// The page's column: at most [`MAX_W`] wide, centred.
fn column(ui: &mut Ui, max: f32, add: impl FnOnce(&mut Ui, f32)) {
    let avail = ui.available_width();
    let w = avail.min(max);
    let pad = ((avail - w) / 2.0).floor();
    let spacing = ui.spacing().item_spacing;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(pad);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = spacing;
            ui.set_width(w);
            add(ui, w);
        });
    });
}

/// The primary button: filled with the accent.
fn primary(ui: &mut Ui, text: &str) -> egui::Response {
    let c = palette::cat();
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = vec2(18.0, 9.0);
        ui.add(
            egui::Button::new(RichText::new(text).size(15.0).color(c.crust))
                .fill(c.mauve)
                .stroke(Stroke::NONE)
                .corner_radius(10),
        )
    })
    .inner
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A quieter button beside [`primary`].
fn secondary(ui: &mut Ui, text: &str) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = vec2(16.0, 9.0);
        ui.add(egui::Button::new(RichText::new(text).size(15.0)).corner_radius(10))
    })
    .inner
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl App {
    /// Home, in the central panel.
    pub(crate) fn home_page(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        self.home_view.poll(ctx);
        let mut act = None;
        if self.history.entries.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("home")
                .auto_shrink(false)
                .show(ui, |ui| welcome(ui, &mut act));
            self.home_act(act, ctx);
            return;
        }
        // Known words: the merged knowledge is made when a book is
        // compared; with none opened yet, made here once.
        if self.known.merged_from.is_none() {
            self.update_merged();
        }
        let known = self.home_known();
        let (sum, set) = self.home_view.summary();
        let books = self.home_books(&sum);
        let ask = books
            .iter()
            .take(1 + SHELF_MAX)
            .map(|b| Ask {
                id: b.id.clone(),
                path: b.path.clone(),
                epub: b.epub,
            })
            .collect();
        self.home_view.request(ask, ctx);
        // Small covers stand in until the large ones are loaded.
        for b in books.iter().take(1 + SHELF_MAX) {
            if !self.covers.contains_key(&b.id) {
                let t = crate::load_cover(ctx, &self.history.cover(&b.id));
                self.covers.insert(b.id.clone(), t);
            }
        }
        let cpm = self.cpm();
        let library_size = self.history.entries.len();
        let view = &mut self.home_view;
        let covers = &self.covers;
        let cover_of = |id: &str| -> Option<&TextureHandle> {
            match view_cover(&view.books, id) {
                Some(t) => Some(t),
                None => covers.get(id).and_then(|t| t.as_ref()),
            }
        };
        let books_info = &view.books;
        let progress = |b: &Shown| -> Option<f32> {
            if b.finished {
                return Some(1.0);
            }
            let at = b.at?;
            let info = books_info.get(&b.id)?.as_ref()?;
            Some(crate::chapter_lengths::through(info.chapters.as_ref()?, at))
        };
        let goal_edit = &mut view.goal_edit;
        egui::ScrollArea::vertical()
            .id_salt("home")
            .auto_shrink(false)
            .show(ui, |ui| {
                column(ui, MAX_W, |ui, w| {
                    ui.add_space(22.0);
                    header(ui, library_size, &mut act);
                    ui.add_space(30.0);
                    let wide = w >= WIDE;
                    if let Some(hero) = books.first() {
                        let info = books_info.get(&hero.id).and_then(|i| i.as_ref());
                        let today_w = if wide { TODAY_W } else { 0.0 };
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            let r = hero_cover(ui, hero, cover_of(&hero.id), &mut act);
                            ui.add_space(36.0);
                            let text_w =
                                (w - r.width() - 36.0 - if wide { today_w + 40.0 } else { 0.0 })
                                    .max(160.0);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing = vec2(8.0, 5.0);
                                ui.set_width(text_w);
                                hero_text(
                                    ui,
                                    hero,
                                    info,
                                    progress(hero),
                                    cpm,
                                    &sum,
                                    &set,
                                    &mut act,
                                );
                            });
                            if wide {
                                ui.add_space(40.0);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing = vec2(8.0, 5.0);
                                    ui.set_width(today_w);
                                    today(ui, &sum, goal_edit, &mut act);
                                });
                            }
                        });
                        if !wide {
                            ui.add_space(SECTION);
                            today(ui, &sum, goal_edit, &mut act);
                        }
                    }
                    let shelf: Vec<&Shown> = books
                        .iter()
                        .skip(1)
                        .filter(|b| b.at.is_some())
                        .take(SHELF_MAX.min(shelf_fits(w)))
                        .collect();
                    if !shelf.is_empty() {
                        ui.add_space(SECTION);
                        let read = books.iter().skip(1).filter(|b| b.at.is_some()).count();
                        if section_head(ui, "Recently read", Some(read), Some("See all ›")) {
                            act = Some(Act::Library(HistTab::Stats));
                        }
                        ui.add_space(14.0);
                        shelf_row(
                            ui, &shelf, &books, &cover_of, &progress, &sum, &set, &mut act,
                        );
                    }
                    ui.add_space(SECTION);
                    year(ui, &sum);
                    ui.add_space(SECTION);
                    totals(ui, &sum, &set, known, w);
                    ui.add_space(SECTION);
                });
            });
        self.home_act_books(act, &books, ctx);
    }

    /// The books Home shows: those read in the reader, the one moved in
    /// last first; or, with none, the book analysed last.
    fn home_books(&self, sum: &Summary) -> Vec<Shown> {
        let finished: HashSet<&str> = sum
            .books
            .iter()
            .filter(|(_, b)| b.finished.is_some())
            .map(|(id, _)| id.as_str())
            .collect();
        let shown = |e: &history::Entry, at: Option<LeftAt>| Shown {
            id: e.id.clone(),
            title: e.title.clone(),
            path: e.path.clone(),
            epub: e.stats.format == "EPUB",
            traditional: e.stats.variant == "Traditional",
            han_total: e.stats.han_total,
            at,
            finished: finished.contains(e.id.as_str()),
        };
        let mut read: Vec<Shown> = self
            .history
            .entries
            .iter()
            .filter(|e| e.stats.format == "EPUB")
            .filter_map(|e| rs::left_at(&e.id).map(|at| shown(e, Some(at))))
            .collect();
        read.sort_by_key(|b| std::cmp::Reverse(b.at.map_or(0, |a| a.saved)));
        if read.is_empty()
            && let Some(e) = self.history.entries.iter().max_by_key(|e| e.analysed)
        {
            read.push(shown(e, None));
        }
        read
    }

    /// Known words and characters (Anki and the lists), counted once for
    /// each new knowledge.
    fn home_known(&mut self) -> Option<(u64, u64)> {
        let k = self.known.merged.as_ref()?;
        let key = Arc::as_ptr(k) as usize;
        if let Some((at, w, c)) = self.home_view.known
            && at == key
        {
            return Some((w, c));
        }
        let w = k.words.values().filter(|s| s.is_known()).count() as u64;
        let c = k.chars.values().filter(|s| s.is_known()).count() as u64;
        self.home_view.known = Some((key, w, c));
        Some((w, c))
    }

    fn home_act_books(&mut self, act: Option<Act>, books: &[Shown], ctx: &egui::Context) {
        match act {
            Some(Act::Read(i)) => {
                let Some(b) = books.get(i) else { return };
                match reader::Reader::open(
                    &b.path,
                    Some(&b.id),
                    b.traditional,
                    ctx,
                    &mut self.reader_dictionary,
                ) {
                    Ok(r) => self.reader = Some(r),
                    Err(e) => self.set_error("reader", e),
                }
            }
            Some(Act::Overview(i)) => {
                let Some(b) = books.get(i) else { return };
                if self.report.is_some() && self.report_id.as_deref() == Some(b.id.as_str()) {
                    self.home = false;
                } else {
                    self.open(b.path.clone(), ctx);
                }
            }
            other => self.home_act(other, ctx),
        }
    }

    fn home_act(&mut self, act: Option<Act>, ctx: &egui::Context) {
        match act {
            Some(Act::Library(tab)) => {
                self.show_history = true;
                self.history_tab = tab;
            }
            Some(Act::Open) => self.open_dialog(ctx),
            Some(Act::Anki) => self.toggle_anki(ctx),
            Some(Act::Known) => self.known.window = true,
            Some(Act::Goal(minutes, chars)) => {
                let today = self.home_view.summary().0.today;
                rs::update_meta(|s| s.set_goal(today, minutes, chars));
                if let Err(e) = rs::save() {
                    self.set_error("reading stats", e);
                }
            }
            Some(Act::Read(_) | Act::Overview(_)) | None => {}
        }
    }
}

fn view_cover<'a>(
    books: &'a HashMap<String, Option<BookInfo>>,
    id: &str,
) -> Option<&'a TextureHandle> {
    books.get(id)?.as_ref()?.cover.as_ref()
}

/// "Good evening", the date, and the Library's size.
fn header(ui: &mut Ui, library_size: usize, act: &mut Option<Act>) {
    let now = rs::now();
    let local = now + rs::utc_offset(now as i64) as f64;
    let secs = local.floor() as i64;
    let day = secs.div_euclid(86_400);
    let hour = secs.rem_euclid(86_400) / 3600;
    let (_, m, d) = rs::civil(day);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(palette::bold_text(greeting(hour), 28.0).color(palette::strong()));
            ui.label(
                RichText::new(format!(
                    "{}, {d} {}",
                    WEEKDAYS[rs::weekday(day as i32) as usize],
                    MONTHS[m as usize - 1]
                ))
                .size(15.0)
                .color(palette::weak()),
            );
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let books = if library_size == 1 { "book" } else { "books" };
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(format!(
                            "📖 {} {books} in your Library ›",
                            fmt_int(library_size as u64)
                        ))
                        .color(palette::weak()),
                    )
                    .frame(false),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                *act = Some(Act::Library(HistTab::Books));
            }
        });
    });
}

/// The hero's cover: a click reads the book (EPUB) or opens its Overview.
fn hero_cover(ui: &mut Ui, b: &Shown, tex: Option<&TextureHandle>, act: &mut Option<Act>) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(HERO_COVER, Sense::click());
    let hovered = ui.rect_contains_pointer(rect);
    let lift = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("lift"), hovered, 0.15);
    paint_cover(
        ui,
        rect.translate(vec2(0.0, -3.0 * lift)),
        tex,
        &b.title,
        &b.id,
        lift,
    );
    let tip = if b.epub { "Read it" } else { "Its Overview" };
    if resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip)
        .clicked()
    {
        *act = Some(if b.epub {
            Act::Read(0)
        } else {
            Act::Overview(0)
        });
    }
    rect
}

#[allow(clippy::too_many_arguments)]
fn hero_text(
    ui: &mut Ui,
    b: &Shown,
    info: Option<&BookInfo>,
    done: Option<f32>,
    cpm: f32,
    sum: &Summary,
    set: &rs::TrackerSettings,
    act: &mut Option<Act>,
) {
    let started = b.at.is_some();
    let kicker = match (started, b.finished) {
        (_, true) => "FINISHED",
        (true, false) => "CONTINUE READING",
        (false, _) => "START READING",
    };
    ui.add_space(6.0);
    ui.label(
        RichText::new(kicker)
            .size(11.5)
            .extra_letter_spacing(1.5)
            .color(palette::highlight()),
    );
    ui.add_space(4.0);
    ui.add(egui::Label::new(palette::bold_text(&b.title, 27.0).color(palette::strong())).wrap());
    ui.add_space(6.0);
    let mut meta: Vec<String> = Vec::new();
    if let (Some(at), Some(info)) = (b.at, info)
        && let Some(name) = chapter_name(&info.toc, at.spine as usize)
    {
        meta.push(name.to_owned());
    }
    if let Some(p) = done {
        if b.finished {
            meta.push("read to the end".to_owned());
        } else {
            meta.push(format!("{} read", percent(p)));
            if let Some(t) = time_left(b.han_total, p, cpm) {
                meta.push(t);
            }
        }
    } else if !started {
        if let Some(t) = time_left(b.han_total, 0.0, cpm) {
            meta.push(t.replace(" left", " to read"));
        }
        meta.push(format!("{} characters", fmt_int(b.han_total)));
    }
    if let Some(at) = b.at {
        meta.push(format!(
            "last read {}",
            ago(rs::day_of(at.saved, set), sum.today)
        ));
    }
    if !meta.is_empty() {
        ui.add(
            egui::Label::new(
                RichText::new(meta.join("  ·  "))
                    .size(14.5)
                    .color(palette::weak()),
            )
            .wrap(),
        );
    }
    if started {
        ui.add_space(16.0);
        let w = ui.available_width().min(460.0);
        let (rect, _) = ui.allocate_exact_size(vec2(w, 6.0), Sense::hover());
        progress_bar(ui, rect, done.unwrap_or(0.0));
    }
    ui.add_space(24.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        if b.epub {
            let label = if started {
                "▶  Continue reading"
            } else {
                "▶  Start reading"
            };
            if primary(ui, label).clicked() {
                *act = Some(Act::Read(0));
            }
            if secondary(ui, "Overview")
                .on_hover_text("The book's figures and words")
                .clicked()
            {
                *act = Some(Act::Overview(0));
            }
        } else if primary(ui, "Overview").clicked() {
            *act = Some(Act::Overview(0));
        }
    });
}

/// Today's reading: the goal as a ring, and the streaks.
fn today(ui: &mut Ui, sum: &Summary, goal_edit: &mut Option<(u32, u64)>, act: &mut Option<Act>) {
    // As the goal measures it: listening folded in when it counts.
    let day = sum.goal_day(&sum.days.get(&sum.today).copied().unwrap_or_default());
    let goal = sum.goal.filter(|g| g.is_set());
    ui.add_space(6.0);
    ui.label(palette::bold_text("Today", 19.0).color(palette::strong()));
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        let minutes = (day.read_secs / 60.0).floor() as u64;
        ring(
            ui,
            goal.map_or(0.0, |g| g.progress(&day)),
            &minutes.to_string(),
            "min",
            &ring_hover(goal.as_ref(), &day, sum.goal_listening),
        );
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.add_space(14.0);
            ui.label(
                RichText::new(format!("{} characters", fmt_int(day.read_chars)))
                    .size(14.0)
                    .color(palette::strong()),
            );
            let goal_text = match goal {
                Some(g) if g.met(&day) => "Goal met 🎉".to_owned(),
                Some(g) => goal_words(&g),
                None => "No daily goal".to_owned(),
            };
            ui.label(RichText::new(goal_text).color(palette::weak()));
            goal_menu(ui, goal.map(|g| (g.minutes, g.chars)), goal_edit, act);
        });
    });
    ui.add_space(16.0);
    let (current, best) = sum.read_streak;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if current > 0 {
            ui.label(
                palette::bold_text(format!("🔥 {current}-day streak"), 14.5)
                    .color(palette::pick(palette::cat().peach, palette::ink::PEACH)),
            );
        } else {
            ui.label(
                RichText::new("No streak yet")
                    .size(14.5)
                    .color(palette::weak()),
            )
            .on_hover_text("Read a minute or more on a day to start one");
        }
        if best > 0 {
            ui.label(
                RichText::new(format!("·  best {best}"))
                    .size(14.5)
                    .color(palette::weak()),
            );
        }
    });
}

/// "45 min a day" or "45 min + 5,000 chars a day".
/// The Today ring's hover text: the minutes in it, and what fills it.
fn ring_hover(goal: Option<&rs::Goal>, day: &rs::Day, listening: bool) -> String {
    let what = if listening {
        "Minutes read and listened to today, in every book."
    } else {
        "Minutes read today, in every book."
    };
    let Some(g) = goal else {
        return format!("{what} Set a daily goal to fill the ring.");
    };
    let mut s = format!(
        "{what} The ring is today's goal: {}% of it",
        (g.progress(day) * 100.0).floor()
    );
    if g.minutes > 0 && g.chars > 0 {
        s.push_str(", following whichever of minutes and characters is further behind");
    }
    s.push_str(if listening {
        ". Read aloud counts (Settings, ⚙)."
    } else {
        ". Read aloud doesn't count."
    });
    s
}

fn goal_words(g: &rs::Goal) -> String {
    let mut what = Vec::new();
    if g.minutes > 0 {
        what.push(format!("{} min", g.minutes));
    }
    if g.chars > 0 {
        what.push(format!("{} chars", fmt_int(g.chars)));
    }
    format!("Goal: {} a day", what.join(" + "))
}

/// The daily goal, set from Home as in the reader's Stats panel.
fn goal_menu(
    ui: &mut Ui,
    now: Option<(u32, u64)>,
    edit: &mut Option<(u32, u64)>,
    act: &mut Option<Act>,
) {
    let label = if now.is_some() {
        "Change goal…"
    } else {
        "Set a goal…"
    };
    let r = ui.menu_button(RichText::new(label).color(palette::accent()), |ui| {
        let e = edit.get_or_insert(now.unwrap_or((30, 0)));
        ui.label(palette::bold_text("Daily goal", 14.0));
        ui.add_space(4.0);
        egui::Grid::new("home_goal")
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("Time");
                ui.add(egui::DragValue::new(&mut e.0).range(0..=600).suffix(" min"));
                ui.end_row();
                ui.label("Characters");
                ui.add(
                    egui::DragValue::new(&mut e.1)
                        .range(0..=200_000)
                        .speed(50.0)
                        .suffix(" chars"),
                );
                ui.end_row();
            });
        ui.label(
            RichText::new("0 turns either off. A change applies from today on.")
                .small()
                .color(palette::weak()),
        );
        ui.add_space(4.0);
        if ui.button("Save").clicked() {
            *act = Some(Act::Goal(e.0, e.1));
            ui.close();
        }
    });
    if r.inner.is_none() {
        *edit = None;
    }
}

/// The shelf: covers with their titles and how far each was read.
#[allow(clippy::too_many_arguments)]
fn shelf_row<'a>(
    ui: &mut Ui,
    shelf: &[&Shown],
    all: &[Shown],
    cover_of: &dyn Fn(&str) -> Option<&'a TextureHandle>,
    progress: &dyn Fn(&Shown) -> Option<f32>,
    sum: &Summary,
    set: &rs::TrackerSettings,
    act: &mut Option<Act>,
) {
    let title_h = 36.0;
    let h = SHELF_COVER.y + 12.0 + 3.0 + 10.0 + title_h + 4.0 + 14.0;
    let w = shelf.len() as f32 * (SHELF_COVER.x + SHELF_GAP) - SHELF_GAP;
    let (area, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let c = palette::cat();
    for (k, b) in shelf.iter().enumerate() {
        let i = all.iter().position(|x| x.id == b.id).unwrap_or(0);
        let x = area.min.x + k as f32 * (SHELF_COVER.x + SHELF_GAP);
        let rect = Rect::from_min_size(pos2(x, area.min.y + 6.0), SHELF_COVER);
        let id = egui::Id::unique(("home shelf", &b.id));
        let resp = ui.interact(rect, id, Sense::click());
        let hovered = ui.rect_contains_pointer(rect);
        let lift = ui
            .ctx()
            .animate_bool_with_time(id.with("lift"), hovered, 0.15);
        let drawn = rect.translate(vec2(0.0, -4.0 * lift));
        paint_cover(ui, drawn, cover_of(&b.id), &b.title, &b.id, lift);
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!("Read {}", b.title))
            .clicked()
        {
            *act = Some(Act::Read(i));
        }
        // The Overview, on the cover while it is pointed at.
        if hovered {
            let pill = Rect::from_center_size(
                pos2(drawn.center().x, drawn.max.y - 22.0),
                vec2(84.0, 24.0),
            );
            let pr = ui.interact(pill, id.with("overview"), Sense::click());
            let fill = if pr.hovered() { c.surface1 } else { c.base };
            ui.painter()
                .rect_filled(pill, 12.0, fill.gamma_multiply(0.94));
            ui.painter().text(
                pill.center(),
                Align2::CENTER_CENTER,
                "Overview",
                FontId::proportional(12.0),
                c.text,
            );
            if pr
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("The book's figures and words")
                .clicked()
            {
                *act = Some(Act::Overview(i));
            }
        }
        // How far, under the cover; then the title, two lines at most.
        let by = rect.max.y + 12.0;
        let done = progress(b);
        progress_bar(
            ui,
            Rect::from_min_size(pos2(x, by), vec2(SHELF_COVER.x, 3.0)),
            done.unwrap_or(0.0),
        );
        let mut job = egui::text::LayoutJob::simple(
            b.title.clone(),
            FontId::proportional(13.0),
            palette::strong(),
            SHELF_COVER.x,
        );
        job.wrap.max_rows = 2;
        job.wrap.break_anywhere = true;
        let g = ui.painter().layout_job(job);
        let ty = by + 3.0 + 10.0;
        let title_end = ty + g.size().y;
        ui.painter().galley(pos2(x, ty), g, palette::strong());
        let mut note = match done {
            Some(p) if b.finished || p >= 0.999 => "Finished".to_owned(),
            Some(p) => percent(p),
            None => String::new(),
        };
        if let Some(at) = b.at {
            let when = ago_short(rs::day_of(at.saved, set), sum.today);
            note = if note.is_empty() {
                when
            } else {
                format!("{note} · {when}")
            };
        }
        ui.painter().text(
            pos2(x, title_end + 4.0),
            Align2::LEFT_TOP,
            note,
            FontId::proportional(11.5),
            palette::weak(),
        );
    }
}

/// This year's reading, a small square a day, weeks across (as in
/// Library Stats); the days still to come are dimmed.
fn year(ui: &mut Ui, sum: &Summary) {
    const SHORT: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (y, _, _) = rs::civil(sum.today as i64);
    let first = rs::days_from_civil(y, 1, 1) as i32;
    let n = rs::days_from_civil(y + 1, 1, 1) as i32 - first;
    let read = rs::sum_range(&sum.days, first, sum.today + 1);
    let days_read = sum
        .days
        .range(first..=sum.today)
        .filter(|(_, d)| d.read_secs >= rs::DAY_READ_SECS)
        .count();
    ui.horizontal(|ui| {
        ui.label(palette::bold_text(y.to_string(), 19.0).color(palette::strong()));
        ui.label(
            RichText::new(if read.read_secs >= 60.0 {
                format!(
                    "{} read on {} {}",
                    mins(read.read_secs),
                    days_read,
                    if days_read == 1 { "day" } else { "days" }
                )
            } else {
                "nothing read yet".to_owned()
            })
            .size(14.5)
            .color(palette::weak()),
        );
    });
    ui.add_space(10.0);
    let months: Vec<(i32, &str)> = (1..=12)
        .map(|m| (rs::days_from_civil(y, m, 1) as i32, SHORT[m as usize - 1]))
        .collect();
    let shades = calendar_shades();
    // Small squares: the calendar's cells grow with its width, so it is
    // given about 11 points a week.
    ui.scope(|ui| {
        ui.set_max_width(YEAR_W.min(ui.available_width()));
        charts::calendar(
            ui,
            charts::Calendar {
                first,
                weekday: rs::weekday(first),
                n,
                months: &months,
                shades,
                ring: palette::good(),
                focus: Some((first, sum.today + 1)),
            },
            &|i| {
                let d = first + i;
                let v = sum.days.get(&d)?;
                let goal = sum.goal_days.contains(&d);
                Some(charts::CalDay {
                    level: day_level(v),
                    ring: goal,
                    hover: day_hover(d, v, goal),
                })
            },
        );
    });
    ui.add_space(8.0);
    let weak = palette::weak();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.label(RichText::new("Less").small().color(weak));
        for s in shades {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().rect_filled(r, 2.0, s);
        }
        ui.label(RichText::new("More").small().color(weak))
            .on_hover_text("Under 15 min, 15–30, 30–60, an hour or more");
        if sum.goal.is_some() || !sum.goal_days.is_empty() {
            ui.add_space(10.0);
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().rect_stroke(
                r.shrink(0.5),
                2.0,
                Stroke::new(1.5, palette::good()),
                StrokeKind::Inside,
            );
            ui.label(RichText::new("goal met").small().color(weak));
        }
    });
}

/// Books finished, time and characters read, and words known.
fn totals(
    ui: &mut Ui,
    sum: &Summary,
    set: &rs::TrackerSettings,
    known: Option<(u64, u64)>,
    w: f32,
) {
    ui.label(palette::bold_text("All time", 19.0).color(palette::strong()));
    ui.add_space(12.0);
    let (y, m, _) = rs::civil(sum.today as i64);
    let month_start = rs::days_from_civil(y, m as i64, 1) as i32;
    let this_month = rs::sum_range(&sum.days, month_start, sum.today + 1);
    let finished_month = sum
        .books
        .iter()
        .filter_map(|(_, b)| b.finished)
        .filter(|&t| rs::day_of(t, set) >= month_start)
        .count();
    let hours = sum.totals.read_secs / 3600.0;
    let time = if sum.totals.read_secs < 3600.0 {
        mins(sum.totals.read_secs)
    } else if hours < 10.0 {
        format!("{hours:.1} h")
    } else {
        format!("{} h", fmt_int(hours.round() as u64))
    };
    let month_note = |s: String| format!("{s} this month");
    let tiles: [(String, &str, String); 4] = [
        (
            fmt_int(sum.books_finished as u64),
            if sum.books_finished == 1 {
                "book finished"
            } else {
                "books finished"
            },
            month_note(finished_month.to_string()),
        ),
        (
            time,
            "reading",
            if this_month.read_secs >= 60.0 {
                month_note(mins(this_month.read_secs))
            } else {
                "none this month".to_owned()
            },
        ),
        (
            charts::compact(sum.totals.read_chars as f64),
            "characters read",
            month_note(charts::compact(this_month.read_chars as f64)),
        ),
        match known {
            Some((words, chars)) => (
                fmt_int(words),
                "words known",
                format!("{} characters", fmt_int(chars)),
            ),
            None => (
                "–".to_owned(),
                "words known",
                "Anki or word lists set it".to_owned(),
            ),
        },
    ];
    let gap = 16.0;
    // In one row when four fit at a readable width, else two by two.
    let per_row = if w >= 4.0 * 180.0 + 3.0 * gap { 4 } else { 2 };
    let tile_w = ((w - gap * (per_row - 1) as f32) / per_row as f32).floor();
    for row in tiles.chunks(per_row) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (value, label, note) in row {
                tile(ui, tile_w, value, label, note);
            }
        });
        ui.add_space(gap - 5.0);
    }
}

fn tile(ui: &mut Ui, w: f32, value: &str, label: &str, note: &str) {
    palette::card()
        .inner_margin(egui::Margin::symmetric(18, 16))
        .show(ui, |ui| {
            ui.set_width(w - 36.0);
            // The card sits in a row: its lines go down.
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(palette::bold_text(value, 26.0).color(palette::strong()));
                ui.label(RichText::new(label).size(14.0).color(palette::strong()));
                ui.label(RichText::new(note).size(12.0).color(palette::weak()));
            });
        });
}

/// An empty Library: a place to drop books, and what to do first.
fn welcome(ui: &mut Ui, act: &mut Option<Act>) {
    column(ui, 760.0, |ui, w| {
        ui.add_space(56.0);
        ui.vertical_centered(|ui| {
            ui.label(palette::bold_text("Welcome to hanstat", 30.0).color(palette::strong()));
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "See how much of a Chinese book you already know, then read it here.",
                )
                .size(15.5)
                .color(palette::weak()),
            );
        });
        ui.add_space(32.0);
        let c = palette::cat();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 210.0), Sense::click());
        let dragging = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let lit = dragging || resp.hovered();
        let p = ui.painter();
        p.rect_filled(
            rect,
            16.0,
            if lit {
                palette::mix(c.mauve, c.base, 0.12)
            } else {
                c.base
            },
        );
        let edge = if lit { c.mauve } else { c.overlay0 };
        let r = rect.shrink(1.0);
        let pts = [
            r.left_top() + vec2(16.0, 0.0),
            r.right_top() - vec2(16.0, 0.0),
            r.right_top() + vec2(0.0, 16.0),
            r.right_bottom() - vec2(0.0, 16.0),
            r.right_bottom() - vec2(16.0, 0.0),
            r.left_bottom() + vec2(16.0, 0.0),
            r.left_bottom() - vec2(0.0, 16.0),
            r.left_top() + vec2(0.0, 16.0),
        ];
        for k in (0..8).step_by(2) {
            p.extend(egui::Shape::dashed_line(
                &[pts[k], pts[k + 1]],
                Stroke::new(1.5, edge),
                8.0,
                6.0,
            ));
        }
        // The rounded corners, as quarter circles.
        for (centre, from) in [
            (r.left_top() + vec2(16.0, 16.0), 2.0),
            (r.right_top() + vec2(-16.0, 16.0), 3.0),
            (r.right_bottom() + vec2(-16.0, -16.0), 0.0),
            (r.left_bottom() + vec2(16.0, -16.0), 1.0),
        ] {
            let arc: Vec<Pos2> = (0..=8)
                .map(|i| {
                    let a = (from + i as f32 / 8.0) * std::f32::consts::FRAC_PI_2;
                    centre + 16.0 * vec2(a.cos(), a.sin())
                })
                .collect();
            p.add(egui::Shape::line(arc, Stroke::new(1.5, edge)));
        }
        let inner = rect.shrink2(vec2(24.0, 36.0));
        // Inside the zone, without moving the page's cursor.
        {
            let ui = &mut ui.new_child(
                UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::top_down(Align::Center)),
            );
            {
                ui.label(RichText::new("📚").size(34.0));
                ui.add_space(6.0);
                let text = if drop_works() {
                    "Drop .epub or .txt files here"
                } else {
                    "Choose .epub or .txt files"
                };
                ui.label(palette::bold_text(text, 18.0).color(palette::strong()));
                ui.add_space(12.0);
                if primary(ui, "📂  Open books…").clicked() {
                    *act = Some(Act::Open);
                }
            }
        }
        if resp.clicked() {
            *act = Some(Act::Open);
        }
        ui.add_space(SECTION);
        let steps: [(&str, &str); 3] = [
            (
                "Open a book",
                "Each one is analysed and kept in your Library, with its figures and words.",
            ),
            (
                "Say what you know",
                "Your Anki cards, HSK or TOCFL levels, or your own word lists.",
            ),
            (
                "Read",
                "Read EPUBs here, with a pop-up dictionary. Your reading time and streaks show on this page.",
            ),
        ];
        let wide = w >= 600.0;
        let gap = 28.0;
        let step_w = if wide {
            ((w - 2.0 * gap) / 3.0).floor()
        } else {
            w
        };
        let mut draw = |ui: &mut Ui, k: usize| {
            let (title, text) = steps[k];
            ui.vertical(|ui| {
                ui.set_width(step_w);
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 13.0, c.mauve);
                    ui.painter().text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        (k + 1).to_string(),
                        palette::bold(14.0),
                        c.crust,
                    );
                    ui.label(palette::bold_text(title, 16.0).color(palette::strong()));
                });
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(RichText::new(text).size(13.5).color(palette::weak())).wrap(),
                );
                if k == 1 {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("🃏 Anki…").clicked() {
                            *act = Some(Act::Anki);
                        }
                        if ui.button("📚 HSK, TOCFL & lists…").clicked() {
                            *act = Some(Act::Known);
                        }
                    });
                }
            });
        };
        if wide {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for k in 0..3 {
                    draw(ui, k);
                }
            });
        } else {
            for k in 0..3 {
                draw(ui, k);
                ui.add_space(18.0);
            }
        }
        ui.add_space(SECTION);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_name_is_the_last_entry_at_or_before() {
        let toc = vec![
            (0, "Cover".to_owned()),
            (2, "Chapter 1".to_owned()),
            (2, "Chapter 1, part 2".to_owned()),
            (5, "Chapter 2".to_owned()),
        ];
        assert_eq!(chapter_name(&toc, 1), Some("Cover"));
        assert_eq!(chapter_name(&toc, 3), Some("Chapter 1"));
        assert_eq!(chapter_name(&toc, 9), Some("Chapter 2"));
        assert_eq!(chapter_name(&[], 3), None);
    }

    #[test]
    fn time_left_rounds_long_times_to_five_minutes() {
        assert_eq!(time_left(0, 0.0, 300.0), None);
        assert_eq!(time_left(1000, 0.0, 0.0), None);
        assert_eq!(time_left(3000, 0.0, 300.0).unwrap(), "about 10 min left");
        assert_eq!(time_left(3000, 0.5, 300.0).unwrap(), "about 5 min left");
        // 250 min → 4 h 10 min; 252 → 4 h 10; 253 → 4 h 15.
        assert_eq!(
            time_left(75_000, 0.0, 300.0).unwrap(),
            "about 4 h 10 min left"
        );
        assert_eq!(
            time_left(75_600, 0.0, 300.0).unwrap(),
            "about 4 h 10 min left"
        );
        assert_eq!(
            time_left(75_900, 0.0, 300.0).unwrap(),
            "about 4 h 15 min left"
        );
        assert_eq!(time_left(72_000, 0.0, 300.0).unwrap(), "about 4 h left");
        assert_eq!(
            time_left(100, 0.999, 300.0).unwrap(),
            "less than a minute left"
        );
    }

    #[test]
    fn shelf_fits_whole_covers() {
        assert_eq!(shelf_fits(100.0), 1);
        assert_eq!(shelf_fits(120.0), 1);
        assert_eq!(shelf_fits(264.0), 2);
        assert_eq!(shelf_fits(MAX_W), 7);
    }

    #[test]
    fn cover_uv_cuts_the_long_side_evenly() {
        let r = cover_uv(vec2(200.0, 200.0), 0.5);
        assert!((r.min.x - 0.25).abs() < 1e-6 && (r.max.x - 0.75).abs() < 1e-6);
        assert_eq!((r.min.y, r.max.y), (0.0, 1.0));
        let r = cover_uv(vec2(100.0, 400.0), 0.5);
        assert!((r.min.y - 0.25).abs() < 1e-6 && (r.max.y - 0.75).abs() < 1e-6);
    }

    #[test]
    fn percent_rounds_down_and_shows_a_start() {
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(0.004), "<1%");
        assert_eq!(percent(0.429), "42%");
        assert_eq!(percent(1.0), "100%");
    }

    #[test]
    fn ago_reads_naturally() {
        assert_eq!(ago(10, 10), "today");
        assert_eq!(ago(9, 10), "yesterday");
        assert_eq!(ago(7, 10), "3 days ago");
        assert_eq!(ago_short(0, 20), "2 wk ago");
        assert_eq!(ago_short(0, 90), "3 mo ago");
    }

    #[test]
    fn greeting_by_hour() {
        assert_eq!(greeting(4), "Good evening");
        assert_eq!(greeting(9), "Good morning");
        assert_eq!(greeting(13), "Good afternoon");
        assert_eq!(greeting(21), "Good evening");
    }

    #[test]
    fn ring_goes_red_to_green() {
        let c = &palette::MOCHA;
        assert_eq!(ring_colour(c, 0.0), c.red);
        assert_eq!(ring_colour(c, 0.45), c.peach);
        assert_eq!(ring_colour(c, 0.75), c.yellow);
        assert_eq!(ring_colour(c, 1.0), c.green);
        assert_eq!(ring_colour(c, 1.7), c.green);
        let mid = ring_colour(c, 0.2);
        assert!(mid != c.red && mid != c.peach);
    }

    #[test]
    fn ring_hover_says_what_fills_it() {
        let day = rs::Day {
            read_secs: 600.0,
            read_chars: 3000,
            ..rs::Day::default()
        };
        assert!(ring_hover(None, &day, false).ends_with("Set a daily goal to fill the ring."));
        let minutes = rs::Goal {
            minutes: 30,
            ..rs::Goal::default()
        };
        let t = ring_hover(Some(&minutes), &day, false);
        assert!(t.contains("today's goal: 33% of it."), "{t}");
        assert!(!t.contains("further behind"), "{t}");
        let both = rs::Goal {
            chars: 10_000,
            ..minutes
        };
        let t = ring_hover(Some(&both), &day, false);
        assert!(t.contains("30% of it, following whichever"), "{t}");
        let t = ring_hover(Some(&minutes), &day, true);
        assert!(t.starts_with("Minutes read and listened to today"), "{t}");
        assert!(t.ends_with("Read aloud counts (Settings, ⚙)."), "{t}");
    }
}
