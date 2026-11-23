//! The image exports: the whole Library as one long PNG (its totals, each
//! language's vocabulary and every book), and one book's Overview. Laid
//! out with egui in an offscreen context in the window's theme and drawn
//! by [`crate::raster`] on a background thread.

use crate::charts::{self, Band, Point};
use crate::library_stats::{Counted, fmt_time, length, pct, reading_minutes, unit};
use crate::raster;
use crate::{App, difficulty_color, difficulty_fill, file_name, fmt_int, mode_badge, palette};
use eframe::egui::{
    self, Align2, Color32, FontFamily, FontId, Rect, RichText, Sense, Stroke, StrokeKind,
    TextureHandle, epaint::Mesh, pos2, text::LayoutJob, vec2,
};
use hanstat::anki::Difficulty;
use hanstat::history;
use hanstat::lang::Language;
use hanstat::script::Variant;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::channel;

/// Page width in points, and what is inside its margins.
const PAGE: f32 = 1200.0;
const MARGIN: i8 = 56;
const CONTENT: f32 = PAGE - 2.0 * MARGIN as f32;
const GAP: f32 = 16.0;
/// Pixels per point: images 1920 pixels wide.
const SCALE: f32 = 1920.0 / PAGE;
const CHART_HEIGHT: f32 = 220.0;
/// Font family of the bold face, where the system has one.
const BOLD: &str = "report-bold";
/// Font family of book titles.
const TITLE: &str = "report-title";

#[derive(Clone, Copy)]
pub(crate) enum ImageExport {
    Library,
    Book,
}

/// The page's colours, for the window's theme.
#[derive(Clone, Copy)]
struct Look {
    bg: Color32,
    card: Color32,
    border: Color32,
    text: Color32,
    weak: Color32,
    /// Tracks and pill backgrounds.
    faint: Color32,
}

impl Look {
    /// The window's Catppuccin flavour.
    fn new(dark: bool) -> Look {
        let (f, weak) = if dark {
            (&palette::MOCHA, palette::ink::WEAK_DARK)
        } else {
            (&palette::LATTE, palette::ink::WEAK)
        };
        Look {
            bg: f.mantle,
            card: f.base,
            border: if dark { f.surface0 } else { mix(f.surface0, f.base, 0.7) },
            text: f.text,
            weak,
            faint: f.surface0,
        }
    }

    fn style(self, s: &mut egui::Style) {
        s.visuals.panel_fill = self.card;
        s.visuals.override_text_color = Some(self.text);
        s.visuals.weak_text_color = Some(self.weak);
        s.visuals.widgets.noninteractive.bg_stroke.color = self.border;
        s.spacing.item_spacing = vec2(GAP, 0.0);
    }
}

/// LXGW WenKai Medium 1.530, for book titles (SIL OFL 1.1; its licence is
/// beside it). It has Chinese, both scripts, and Japanese; Hangul comes
/// from the bold family.
pub(crate) static WENKAI: &[u8] = include_bytes!("../assets/fonts/LXGWWenKai-Medium.ttf");

/// The page's two extra families. Bold: the system's bold CJK faces
/// (Windows), ahead of the regular fonts for what they lack; elsewhere the
/// regular ones. Book titles: the bundled LXGW WenKai, then the bold family.
fn add_fonts(defs: &mut egui::FontDefinitions) {
    let mut bold_family = Vec::new();
    #[cfg(windows)]
    {
        let dir = std::env::var_os("WINDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"))
            .join("Fonts");
        for (name, file) in [
            ("report-yahei-bold", "msyhbd.ttc"),
            ("report-malgun-bold", "malgunbd.ttf"),
        ] {
            if let Ok(bytes) = std::fs::read(dir.join(file)) {
                defs.font_data
                    .insert(name.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
                bold_family.push(name.to_owned());
            }
        }
    }
    bold_family.extend(
        defs.families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    // Hangul in regular text. The window finds it through eframe's system
    // font provider, which the page's own context has not got; macOS and
    // Linux have it in the window's font list.
    #[cfg(windows)]
    if let Some(dir) = std::env::var_os("WINDIR") {
        let path = std::path::PathBuf::from(dir).join(r"Fonts\malgun.ttf");
        if let Ok(bytes) = std::fs::read(path) {
            defs.font_data.insert(
                "report-malgun".to_owned(),
                Arc::new(egui::FontData::from_owned(bytes)),
            );
            for fam in [FontFamily::Proportional, FontFamily::Monospace] {
                defs.families
                    .entry(fam)
                    .or_default()
                    .push("report-malgun".to_owned());
            }
            bold_family.push("report-malgun".to_owned());
        }
    }
    defs.font_data.insert(
        "report-wenkai".to_owned(),
        Arc::new(egui::FontData::from_static(WENKAI)),
    );
    let mut title_family = vec!["report-wenkai".to_owned()];
    title_family.extend(bold_family.iter().cloned());
    defs.families
        .insert(FontFamily::Name(BOLD.into()), bold_family);
    defs.families
        .insert(FontFamily::Name(TITLE.into()), title_family);
}

/// A book's title.
fn title_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(TITLE.into()))
}

fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}

fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

fn bold_text(s: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(s)
        .size(size)
        .family(FontFamily::Name(BOLD.into()))
        .color(color)
}

/// `fg` laid over `bg` at `a`, as an opaque colour.
fn mix(fg: Color32, bg: Color32, a: f32) -> Color32 {
    let m = |f: u8, b: u8| (f as f32 * a + b as f32 * (1.0 - a)).round() as u8;
    Color32::from_rgb(m(fg.r(), bg.r()), m(fg.g(), bg.g()), m(fg.b(), bg.b()))
}

/// Text cut with "…" after `rows` lines of `width`.
fn galley(
    p: &egui::Painter,
    s: &str,
    font: FontId,
    color: Color32,
    width: f32,
    rows: usize,
) -> Arc<egui::Galley> {
    let mut job = LayoutJob::simple(s.to_owned(), font, color, width);
    job.wrap.max_rows = rows;
    job.wrap.overflow_character = Some('…');
    p.layout_job(job)
}

/// Small spaced capitals.
fn caps(p: &egui::Painter, pos: egui::Pos2, s: &str, color: Color32) -> Rect {
    let mut job = LayoutJob::default();
    job.append(
        &s.to_uppercase(),
        0.0,
        egui::TextFormat {
            font_id: bold(11.5),
            color,
            extra_letter_spacing: 1.2,
            ..Default::default()
        },
    );
    let g = p.layout_job(job);
    let r = Rect::from_min_size(pos, g.size());
    p.galley(pos, g, color);
    r
}

/// A rounded label; returns its width.
fn pill(
    p: &egui::Painter,
    left_center: egui::Pos2,
    s: &str,
    size: f32,
    fg: Color32,
    bg: Color32,
) -> f32 {
    let g = p.layout_no_wrap(s.to_owned(), bold(size), fg);
    let pad = vec2(size * 0.75, size * 0.35);
    let r = Rect::from_min_size(
        pos2(left_center.x, left_center.y - g.size().y / 2.0 - pad.y),
        g.size() + 2.0 * pad,
    );
    p.rect_filled(r, r.height() / 2.0, bg);
    p.galley(r.min + pad, g, fg);
    r.width()
}

/// A row of pills in the flow of `ui`.
fn pills(ui: &mut egui::Ui, items: &[(String, Color32, Color32)], size: f32) {
    let h = size * 1.3 + size * 0.7;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter();
    let mut x = rect.left();
    for (s, fg, bg) in items {
        x += pill(p, pos2(x, rect.center().y), s, size, *fg, *bg) + 8.0;
    }
}

fn card(p: &egui::Painter, r: Rect, look: Look) {
    p.add(
        egui::epaint::Shadow {
            offset: [0, 2],
            blur: 10,
            spread: 0,
            color: Color32::from_black_alpha(18),
        }
        .as_shape(r, 14),
    );
    p.rect(
        r,
        14.0,
        look.card,
        Stroke::new(1.0, look.border),
        StrokeKind::Inside,
    );
}

fn frame(look: Look) -> egui::Frame {
    egui::Frame::NONE
        .fill(look.card)
        .stroke(Stroke::new(1.0, look.border))
        .corner_radius(14)
        .inner_margin(20)
        .shadow(egui::epaint::Shadow {
            offset: [0, 2],
            blur: 10,
            spread: 0,
            color: Color32::from_black_alpha(18),
        })
}

/// The colour band across the top of the page.
fn ribbon(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(PAGE, 7.0), Sense::hover());
    let stops = [
        Color32::from_rgb(70, 140, 230),
        Color32::from_rgb(140, 95, 215),
        Color32::from_rgb(225, 85, 120),
        Color32::from_rgb(240, 165, 60),
    ];
    let mut m = Mesh::default();
    let n = stops.len() - 1;
    for (i, c) in stops.iter().enumerate() {
        let x = r.left() + r.width() * i as f32 / n as f32;
        m.colored_vertex(pos2(x, r.top()), *c);
        m.colored_vertex(pos2(x, r.bottom()), *c);
    }
    for i in 0..n as u32 {
        let k = i * 2;
        m.add_triangle(k, k + 1, k + 2);
        m.add_triangle(k + 1, k + 3, k + 2);
    }
    ui.painter().add(egui::Shape::mesh(m));
}

/// The page: the ribbon, then `body` inside the margins, then the footer.
fn page(ui: &mut egui::Ui, look: Look, body: impl FnOnce(&mut egui::Ui)) {
    ribbon(ui);
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: MARGIN,
            right: MARGIN,
            top: 44,
            bottom: 40,
        })
        .show(ui, |ui| {
            ui.set_width(CONTENT);
            body(ui);
            ui.add_space(36.0);
            let (r, _) = ui.allocate_exact_size(vec2(CONTENT, 1.0), Sense::hover());
            ui.painter()
                .hline(r.x_range(), r.center().y, Stroke::new(1.0, look.border));
            ui.add_space(14.0);
            let (r, _) = ui.allocate_exact_size(vec2(CONTENT, 18.0), Sense::hover());
            let p = ui.painter();
            p.text(
                r.left_center(),
                Align2::LEFT_CENTER,
                "hanstat",
                bold(13.0),
                look.weak,
            );
            p.text(
                r.right_center(),
                Align2::RIGHT_CENTER,
                format!(
                    "{} · {}",
                    hanstat::update::version_string(),
                    history::date(history::now())
                ),
                regular(13.0),
                look.weak,
            );
        });
}

/// Caps over a large title and a line under it.
fn header(ui: &mut egui::Ui, look: Look, kicker: &str, title: &str, sub: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(CONTENT, 16.0), Sense::hover());
    caps(ui.painter(), r.left_top(), kicker, palette::accent());
    ui.add_space(6.0);
    ui.label(bold_text(title, 40.0, look.text));
    ui.add_space(6.0);
    ui.label(RichText::new(sub).size(16.0).color(look.weak));
}

fn section(ui: &mut egui::Ui, look: Look, title: &str, sub: &str) {
    ui.add_space(34.0);
    ui.label(bold_text(title, 22.0, look.text));
    if !sub.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new(sub).size(14.0).color(look.weak));
    }
    ui.add_space(14.0);
}

struct Tile {
    title: String,
    value: String,
    color: Option<Color32>,
    sub: String,
    /// The value is a book's title.
    book: bool,
}

fn tile(title: &str, value: impl Into<String>, sub: impl Into<String>) -> Tile {
    Tile {
        title: title.to_owned(),
        value: value.into(),
        color: None,
        sub: sub.into(),
        book: false,
    }
}

impl Tile {
    fn book(mut self) -> Tile {
        self.book = true;
        self
    }

    fn color(mut self, c: Color32) -> Tile {
        self.color = Some(c);
        self
    }
}

/// Tiles in rows of `cols`.
fn tiles(ui: &mut egui::Ui, look: Look, cols: usize, tiles: &[Tile]) {
    const H: f32 = 112.0;
    let w = (CONTENT - GAP * (cols - 1) as f32) / cols as f32;
    for row in tiles.chunks(cols) {
        let (rect, _) = ui.allocate_exact_size(vec2(CONTENT, H), Sense::hover());
        let p = ui.painter();
        for (i, t) in row.iter().enumerate() {
            let r = Rect::from_min_size(
                pos2(rect.left() + i as f32 * (w + GAP), rect.top()),
                vec2(w, H),
            );
            card(p, r, look);
            let inner = r.shrink2(vec2(20.0, 18.0));
            caps(p, inner.left_top(), &t.title, look.weak);
            let font = if t.book { title_font(29.0) } else { bold(29.0) };
            let g = galley(
                p,
                &t.value,
                font,
                t.color.unwrap_or(look.text),
                inner.width(),
                1,
            );
            p.galley(pos2(inner.left(), inner.top() + 22.0), g, look.text);
            let g = galley(p, &t.sub, regular(13.0), look.weak, inner.width(), 1);
            p.galley(
                pos2(inner.left(), inner.bottom() - g.size().y),
                g,
                look.weak,
            );
        }
        ui.add_space(GAP);
    }
}

/// Columns for `n` tiles: rows filled where it can.
fn cols_for(n: usize) -> usize {
    match n {
        0..=4 => n.max(1),
        5 | 10 => 5,
        6 | 9 => 3,
        _ => 4,
    }
}

/// A titled card that fills the width it is given. Returns the height of
/// its contents.
fn chart_card(
    ui: &mut egui::Ui,
    look: Look,
    title: &str,
    sub: &str,
    add: impl FnOnce(&mut egui::Ui),
) -> f32 {
    sized_card(ui, look, title, sub, None, |ui, _| add(ui))
}

/// A titled card whose contents are `height` tall (as returned by
/// [`chart_card`] for a card beside it), so the two line up; `add` is given
/// the height left under the title.
fn sized_card(
    ui: &mut egui::Ui,
    look: Look,
    title: &str,
    sub: &str,
    height: Option<f32>,
    add: impl FnOnce(&mut egui::Ui, f32),
) -> f32 {
    frame(look)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let top = ui.cursor().top();
            ui.label(bold_text(title, 16.0, look.text));
            ui.add_space(3.0);
            ui.label(RichText::new(sub).size(12.5).color(look.weak));
            ui.add_space(12.0);
            let left = height.map_or(CHART_HEIGHT, |h| h - (ui.cursor().top() - top));
            add(ui, left.max(CHART_HEIGHT));
            ui.min_rect().height()
        })
        .inner
}

/// Books by difficulty, as one bar with a legend.
fn difficulty_bar(ui: &mut egui::Ui, look: Look, counts: &[(Difficulty, usize)]) {
    let total: usize = counts.iter().map(|c| c.1).sum();
    if total == 0 {
        return;
    }
    let (r, _) = ui.allocate_exact_size(vec2(CONTENT, 30.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, 8.0, look.faint);
    let mut x = r.left();
    for &(d, n) in counts {
        if n == 0 {
            continue;
        }
        let seg = Rect::from_min_max(
            pos2(x, r.top()),
            pos2(x + r.width() * n as f32 / total as f32, r.bottom()),
        );
        p.rect_filled(seg.shrink2(vec2(1.5, 0.0)), 8.0, difficulty_fill(d));
        if seg.width() > 28.0 {
            p.text(
                seg.center(),
                Align2::CENTER_CENTER,
                n.to_string(),
                bold(14.0),
                Color32::from_black_alpha(220),
            );
        }
        x = seg.right();
    }
    ui.add_space(10.0);
    let (r, _) = ui.allocate_exact_size(vec2(CONTENT, 18.0), Sense::hover());
    let p = ui.painter();
    let mut x = r.left();
    for &(d, n) in counts {
        let dot = Rect::from_center_size(pos2(x + 5.0, r.center().y), vec2(10.0, 10.0));
        p.rect_filled(dot, 3.0, difficulty_fill(d));
        let t = p.text(
            pos2(x + 16.0, r.center().y),
            Align2::LEFT_CENTER,
            format!("{} {n}", d.label()),
            regular(13.0),
            look.weak,
        );
        x = t.right() + 18.0;
    }
}

const DIFFICULTIES: [Difficulty; 5] = [
    Difficulty::Easy,
    Difficulty::Moderate,
    Difficulty::Challenging,
    Difficulty::Hard,
    Difficulty::VeryHard,
];

/// The difficulty bands from `lo` to 100%, with a marker at `p`.
fn gauge(ui: &mut egui::Ui, look: Look, p_known: f64, counted: f64) {
    let lo = ((counted.min(p_known) - 3.0).min(75.0) / 5.0)
        .floor()
        .max(0.0)
        * 5.0;
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 62.0), Sense::hover());
    let track = Rect::from_min_max(
        pos2(r.left(), r.top() + 24.0),
        pos2(r.right(), r.top() + 36.0),
    );
    let x =
        |v: f64| track.left() + ((v - lo) / (100.0 - lo)).clamp(0.0, 1.0) as f32 * track.width();
    let p = ui.painter();
    let mut hi = 100.0;
    let bands = Difficulty::BANDS
        .iter()
        .copied()
        .chain([(lo, Difficulty::VeryHard)]);
    for (b_lo, d) in bands {
        let b_lo = b_lo.max(lo);
        if b_lo >= hi {
            continue;
        }
        let seg = Rect::from_min_max(pos2(x(b_lo), track.top()), pos2(x(hi), track.bottom()));
        p.rect_filled(
            seg.shrink2(vec2(1.0, 0.0)),
            4.0,
            mix(difficulty_fill(d), look.card, 0.85),
        );
        if seg.width() > 70.0 {
            p.text(
                pos2(seg.center().x, track.bottom() + 6.0),
                Align2::CENTER_TOP,
                d.label(),
                regular(12.0),
                look.weak,
            );
        }
        p.text(
            pos2(seg.left(), track.top() - 4.0),
            Align2::CENTER_BOTTOM,
            format!("{b_lo:.0}%"),
            regular(11.0),
            look.weak,
        );
        hi = b_lo;
    }
    p.text(
        pos2(track.right(), track.top() - 4.0),
        Align2::RIGHT_BOTTOM,
        "100%",
        regular(11.0),
        look.weak,
    );
    let mx = x(counted);
    let d = Difficulty::from_percent(counted);
    p.circle(
        pos2(mx, track.center().y),
        9.0,
        look.card,
        Stroke::new(4.0, difficulty_fill(d)),
    );
}

/// Cover textures, loaded on the page's own context.
type Covers = HashMap<String, TextureHandle>;

fn texture(ctx: &egui::Context, name: &str, img: image::DynamicImage) -> TextureHandle {
    let img = img.into_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    ctx.load_texture(
        name,
        egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()),
        egui::TextureOptions::LINEAR,
    )
}

/// The cover fitted in `bx`, centred, with rounded corners and a shadow;
/// without one, the title's first letter on the language's colour.
fn cover(
    ui: &egui::Ui,
    look: Look,
    bx: Rect,
    tex: Option<&TextureHandle>,
    title: &str,
    color: Color32,
) {
    let p = ui.painter();
    let r = match tex {
        Some(t) => {
            let [tw, th] = t.size().map(|x| x as f32);
            let k = (bx.width() / tw).min(bx.height() / th);
            Rect::from_center_size(bx.center(), vec2(tw * k, th * k))
        }
        None => bx,
    };
    p.add(
        egui::epaint::Shadow {
            offset: [0, 3],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(60),
        }
        .as_shape(r, 6),
    );
    match tex {
        Some(t) => egui::Image::new(t).corner_radius(6).paint_at(ui, r),
        None => {
            p.rect_filled(r, 6.0, mix(color, look.card, 0.22));
            let first: String = title.chars().take(1).collect();
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                first,
                bold(r.width() * 0.42),
                mix(color, look.text, 0.6),
            );
        }
    }
}

fn variant_of(s: &history::Stats) -> Option<Variant> {
    match s.variant.as_str() {
        "Simplified" => Some(Variant::Simplified),
        "Traditional" => Some(Variant::Traditional),
        "Mixed" => Some(Variant::Mixed),
        _ => None,
    }
}

/// White text on the language's colour.
fn lang_pill(lang: Language, variant: Option<Variant>) -> (String, Color32, Color32) {
    let (badge, color) = mode_badge(lang, variant);
    (badge.to_owned(), palette::cat().crust, color)
}

/// `(percent, n)` points of a frequency list's cumulative coverage, at
/// about 160 ranks spread evenly on a log scale.
fn coverage_points(counts: &[u32]) -> Vec<(usize, f64)> {
    let total: u64 = counts.iter().map(|&c| c as u64).sum();
    if total == 0 {
        return Vec::new();
    }
    let n = counts.len();
    let mut ranks: Vec<usize> = (0..=160)
        .map(|i| (n as f64).powf(i as f64 / 160.0).round() as usize)
        .collect();
    ranks.dedup();
    let mut out = Vec::new();
    let (mut sum, mut i) = (0u64, 0usize);
    for r in ranks {
        while i < r.min(n) {
            sum += counts[i] as u64;
            i += 1;
        }
        out.push((r.max(1), sum as f64 * 100.0 / total as f64));
    }
    out
}

impl App {
    /// Ask where to save, lay out the page now and draw it in the background.
    pub(crate) fn export_image(&mut self, ctx: &egui::Context, what: ImageExport) {
        let name = match what {
            ImageExport::Library => "hanstat library.png".to_owned(),
            ImageExport::Book => format!(
                "{}.png",
                self.path
                    .as_deref()
                    .map(crate::file_stem)
                    .unwrap_or_else(|| "hanstat".into())
            ),
        };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(name)
            .add_filter("PNG", &["png"])
            .save_file()
        else {
            return;
        };
        self.load_read_marks();
        let look = Look::new(ctx.theme() == egui::Theme::Dark);
        let scene = raster::lay_out(
            ctx,
            PAGE,
            SCALE,
            look.bg,
            add_fonts,
            |s| look.style(s),
            |c| self.covers_for(c, what),
            |ui, covers| match what {
                ImageExport::Library => self.library_page(ui, look, covers),
                ImageExport::Book => self.book_page(ui, look, covers),
            },
        );
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let msg = match scene.write_png(&path) {
                Ok(()) => {
                    let [w, h] = scene.size();
                    format!("Saved {} ({w} × {h})", file_name(&path))
                }
                Err(e) => format!("Save failed: {e}"),
            };
            let _ = tx.send(msg);
            repaint.request_repaint();
        });
        self.image_job = Some(rx);
        self.toast = Some(("Saving the image…".into(), ctx.input(|i| i.time) + 600.0));
    }

    /// The message of a finished image export.
    pub(crate) fn poll_image(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.image_job else {
            return;
        };
        match rx.try_recv() {
            Ok(m) => {
                self.toast = Some((m, ctx.input(|i| i.time) + 5.0));
                self.image_job = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(_) => self.image_job = None,
        }
    }

    pub(crate) fn image_busy(&self) -> bool {
        self.image_job.is_some()
    }

    fn covers_for(&self, ctx: &egui::Context, what: ImageExport) -> Covers {
        let mut out = Covers::new();
        match what {
            ImageExport::Library => {
                for e in &self.history.entries {
                    if let Ok(img) = image::open(self.history.cover(&e.id)) {
                        out.insert(e.id.clone(), texture(ctx, &e.id, img));
                    }
                }
            }
            ImageExport::Book => {
                // The book's own cover, sharper than the Library's thumbnail.
                let full = self
                    .book
                    .as_ref()
                    .and_then(|b| b.cover.as_deref())
                    .and_then(|b| image::load_from_memory(b).ok())
                    .map(|i| i.thumbnail(480, 680));
                let img = full.or_else(|| {
                    let id = self.report_id.as_deref()?;
                    image::open(self.history.cover(id)).ok()
                });
                if let Some(img) = img {
                    out.insert(String::new(), texture(ctx, "cover", img));
                }
            }
        }
        out
    }

    fn library_page(&self, ui: &mut egui::Ui, look: Look, covers: &Covers) {
        let sp = &self.settings;
        let speeds = (sp.cpm as f64, sp.ja_cpm as f64, sp.wpm as f64);
        let entries = &self.history.entries;
        let read_at = self.read_dates();
        let mut langs: Vec<(Language, usize)> = Language::ALL
            .iter()
            .map(|&l| (l, entries.iter().filter(|e| e.stats.language == l).count()))
            .filter(|x| x.1 > 0)
            .collect();
        langs.sort_by_key(|x| std::cmp::Reverse(x.1));

        page(ui, look, |ui| {
            let books = entries.len();
            let lang_list = langs
                .iter()
                .map(|(l, n)| format!("{n} {}", l.name()))
                .collect::<Vec<_>>()
                .join(" · ");
            header(
                ui,
                look,
                "hanstat · Library",
                "My reading library",
                &format!(
                    "{} {} · {lang_list}",
                    fmt_int(books as u64),
                    if books == 1 { "book" } else { "books" }
                ),
            );

            section(
                ui,
                look,
                "At a glance",
                "Every book's last analysis, all languages together",
            );
            let han: u64 = entries.iter().map(|e| e.stats.han_total).sum();
            let kana: u64 = entries.iter().map(|e| e.stats.kana_total).sum();
            let words: u64 = entries.iter().map(|e| e.stats.words_total).sum();
            let minutes: f64 = entries
                .iter()
                .map(|e| reading_minutes(&e.stats, speeds))
                .sum();
            let read = entries
                .iter()
                .filter(|e| read_at.contains_key(&e.id))
                .count();
            let mut t = vec![
                tile(
                    "Books",
                    fmt_int(books as u64),
                    format!("{} languages", langs.len()),
                ),
                tile(
                    "Characters",
                    fmt_int(han),
                    if kana > 0 {
                        format!("Chinese characters and kanji, + {} kana", fmt_int(kana))
                    } else {
                        "Chinese characters".to_owned()
                    },
                ),
                tile("Words", fmt_int(words), "running words, as segmented"),
                tile(
                    "Reading time",
                    fmt_time(minutes),
                    format!("{:.0} days of reading 1 h a day", minutes / 60.0),
                ),
                tile(
                    "Books read",
                    fmt_int(read as u64),
                    format!("{:.0}% of the library", pct(read, books)),
                )
                .color(if read > 0 { palette::good() } else { look.text }),
            ];
            if let Some(e) = entries.iter().max_by_key(|e| length(&e.stats)) {
                t.push(
                    tile(
                        "Longest book",
                        e.title.clone(),
                        format!("{} {}", fmt_int(length(&e.stats)), unit(e.stats.language)),
                    )
                    .book(),
                );
            }
            tiles(ui, look, 3, &t);

            for &(lang, _) in &langs {
                self.language_block(ui, look, lang, covers, &read_at, speeds);
            }
        });
    }

    fn language_block(
        &self,
        ui: &mut egui::Ui,
        look: Look,
        lang: Language,
        covers: &Covers,
        read_at: &HashMap<String, u64>,
        speeds: (f64, f64, f64),
    ) {
        let books: Vec<&history::Entry> = self
            .history
            .entries
            .iter()
            .filter(|e| e.stats.language == lang)
            .collect();
        let counted: Option<&Counted> = self.stats.shown.get(&lang);
        let source = counted.and_then(|c| c.source.as_deref());

        // The language's heading: its badge and name.
        ui.add_space(46.0);
        let (r, _) = ui.allocate_exact_size(vec2(CONTENT, 44.0), Sense::hover());
        let p = ui.painter();
        // The script's badge when every book has the same, else the language's.
        let first = books.first().and_then(|e| variant_of(&e.stats));
        let variant = books
            .iter()
            .all(|e| variant_of(&e.stats) == first)
            .then_some(first)
            .flatten();
        let (_, fg, bg) = lang_pill(lang, variant);
        let w = pill(
            p,
            r.left_center(),
            mode_badge(lang, variant).0,
            18.0,
            fg,
            bg,
        );
        p.text(
            pos2(r.left() + w + 14.0, r.center().y),
            Align2::LEFT_CENTER,
            lang.name(),
            bold(30.0),
            look.text,
        );
        p.text(
            r.right_center(),
            Align2::RIGHT_CENTER,
            format!(
                "{} {}",
                books.len(),
                if books.len() == 1 { "book" } else { "books" }
            ),
            regular(16.0),
            look.weak,
        );
        ui.add_space(6.0);
        if let Some(s) = source {
            ui.label(
                RichText::new(format!("Known now: {s}"))
                    .size(14.0)
                    .color(look.weak),
            );
        }
        ui.add_space(16.0);

        // Size, then vocabulary.
        let han: u64 = books.iter().map(|e| e.stats.han_total).sum();
        let kana: u64 = books.iter().map(|e| e.stats.kana_total).sum();
        let words: u64 = books.iter().map(|e| e.stats.words_total).sum();
        let minutes: f64 = books
            .iter()
            .map(|e| reading_minutes(&e.stats, speeds))
            .sum();
        let read = books.iter().filter(|e| read_at.contains_key(&e.id)).count();
        let mut t = Vec::new();
        if lang != Language::Korean {
            t.push(tile(
                if lang == Language::Japanese {
                    "Kanji"
                } else {
                    "Characters"
                },
                fmt_int(han),
                if lang == Language::Japanese {
                    format!("+ {} kana", fmt_int(kana))
                } else {
                    format!("{} per book", fmt_int(han / books.len() as u64))
                },
            ));
        }
        t.push(tile(
            "Words",
            fmt_int(words),
            format!("{} per book", fmt_int(words / books.len() as u64)),
        ));
        t.push(tile(
            "Reading time",
            fmt_time(minutes),
            "at the speeds set in hanstat",
        ));
        t.push(
            tile(
                "Read",
                format!("{read} of {}", books.len()),
                "books marked read",
            )
            .color(if read > 0 { palette::good() } else { look.text }),
        );
        if let Some(c) = counted {
            let w = &c.words;
            t.push(tile(
                "Distinct words",
                fmt_int(w.vocab as u64),
                format!(
                    "{} in one book only",
                    fmt_int(w.spread.first().copied().unwrap_or(0) as u64)
                ),
            ));
            if lang != Language::Korean {
                t.push(tile(
                    if lang == Language::Japanese {
                        "Distinct kanji"
                    } else {
                        "Distinct characters"
                    },
                    fmt_int(c.chars.unique as u64),
                    format!("{} in all", fmt_int(c.chars.total)),
                ));
            }
            t.push(tile(
                "Words for 98%",
                fmt_int(w.cover_at[3] as u64),
                format!("{} for 95%", fmt_int(w.cover_at[2] as u64)),
            ));
            if source.is_some() {
                let d = Difficulty::from_percent(w.known_running);
                t.push(
                    tile(
                        "Running words known",
                        format!("{:.1}%", w.known_running),
                        format!("{}, across every book", d.label()),
                    )
                    .color(difficulty_color(d)),
                );
                t.push(tile(
                    "Words known",
                    format!("{:.1}%", pct(w.known_words, w.vocab)),
                    format!(
                        "{} of {}",
                        fmt_int(w.known_words as u64),
                        fmt_int(w.vocab as u64)
                    ),
                ));
                if lang != Language::Korean {
                    t.push(tile(
                        if lang == Language::Japanese {
                            "Kanji known"
                        } else {
                            "Characters known"
                        },
                        format!("{:.1}%", pct(c.chars.known, c.chars.unique)),
                        format!(
                            "{} of {}",
                            fmt_int(c.chars.known as u64),
                            fmt_int(c.chars.unique as u64)
                        ),
                    ));
                }
            }
        }
        tiles(ui, look, cols_for(t.len()), &t);

        // How hard the books are now.
        let known_now: HashMap<&str, f64> = match (counted, source.is_some()) {
            (Some(c), true) => c
                .ids
                .iter()
                .zip(&c.words.book_known)
                .map(|(id, &p)| (id.as_str(), p))
                .collect(),
            _ => HashMap::new(),
        };
        if !known_now.is_empty() {
            let mut counts = [0usize; 5];
            for (id, &p) in &known_now {
                counts[Difficulty::from_percent(p - self.style_offset(id)) as usize] += 1;
            }
            ui.add_space(8.0);
            let (r, _) = ui.allocate_exact_size(vec2(CONTENT, 16.0), Sense::hover());
            caps(ui.painter(), r.left_top(), "Difficulty now", look.weak);
            ui.add_space(8.0);
            let parts: Vec<(Difficulty, usize)> = DIFFICULTIES
                .iter()
                .map(|&d| (d, counts[d as usize]))
                .collect();
            difficulty_bar(ui, look, &parts);
            ui.add_space(GAP + 8.0);
        }

        // Charts, two side by side.
        if let Some(c) = counted {
            ui.columns(2, |cols| {
                let w = &c.words;
                let marks: Vec<(f64, usize)> = hanstat::plan::stats::COVER_AT
                    .iter()
                    .copied()
                    .zip(w.cover_at)
                    .filter(|(p, _)| *p >= 90.0)
                    .collect();
                let level = source.is_some().then(|| {
                    (
                        w.known_running,
                        format!("You know {:.1}% of the running words", w.known_running),
                    )
                });
                chart_card(
                    &mut cols[0],
                    look,
                    "Vocabulary coverage",
                    "How much of all the books the most frequent words make up (log scale)",
                    |ui| {
                        charts::coverage_curve(
                            ui,
                            &w.coverage,
                            &marks,
                            level,
                            palette::accent(),
                            "words",
                            CHART_HEIGHT,
                        )
                    },
                );
                if known_now.is_empty() {
                    let lens: Vec<charts::Bar> = {
                        let mut v: Vec<&&history::Entry> = books.iter().collect();
                        v.sort_by_key(|e| std::cmp::Reverse(length(&e.stats)));
                        v.iter()
                            .map(|e| charts::Bar {
                                value: length(&e.stats) as f64,
                                color: palette::accent(),
                                hover: String::new(),
                            })
                            .collect()
                    };
                    chart_card(
                        &mut cols[1],
                        look,
                        &format!("Book lengths ({})", unit(lang)),
                        "Longest first",
                        |ui| charts::columns(ui, &lens, None, CHART_HEIGHT, &charts::compact),
                    );
                } else {
                    let points: Vec<Point> = books
                        .iter()
                        .filter_map(|e| {
                            let p = *known_now.get(e.id.as_str())?;
                            let d = Difficulty::from_percent(p - self.style_offset(&e.id));
                            Some(Point {
                                x: length(&e.stats).max(1) as f64,
                                y: p,
                                color: difficulty_fill(d),
                                hover: String::new(),
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
                    chart_card(
                        &mut cols[1],
                        look,
                        "Length and difficulty",
                        &format!(
                            "Running words known now against length in {} (log scale)",
                            unit(lang)
                        ),
                        |ui| charts::scatter(ui, &points, &bands, (y_lo, 100.0), CHART_HEIGHT),
                    );
                }
            });
        }

        // Every book.
        ui.add_space(26.0);
        let (r, _) = ui.allocate_exact_size(vec2(CONTENT, 16.0), Sense::hover());
        caps(
            ui.painter(),
            r.left_top(),
            &format!("{} books", lang.name()),
            look.weak,
        );
        ui.add_space(12.0);
        let style_on = self.style_on();
        let scores: HashMap<&str, f32> = if style_on {
            self.style.store.current()
        } else {
            HashMap::new()
        };
        let scale = self.style_scale();
        const CARD_H: f32 = 156.0;
        let w = (CONTENT - GAP) / 2.0;
        for pair in books.chunks(2) {
            let (row, _) = ui.allocate_exact_size(vec2(CONTENT, CARD_H), Sense::hover());
            for (i, e) in pair.iter().enumerate() {
                let r = Rect::from_min_size(
                    pos2(row.left() + i as f32 * (w + GAP), row.top()),
                    vec2(w, CARD_H),
                );
                let style = scores
                    .get(e.id.as_str())
                    .map(|&x| format!("Style {}", scale.pct(x)));
                self.book_card(
                    ui,
                    look,
                    r,
                    e,
                    covers.get(&e.id),
                    read_at.get(&e.id).copied(),
                    style,
                    speeds,
                );
            }
            ui.add_space(GAP);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn book_card(
        &self,
        ui: &egui::Ui,
        look: Look,
        r: Rect,
        e: &history::Entry,
        tex: Option<&TextureHandle>,
        read: Option<u64>,
        style: Option<String>,
        speeds: (f64, f64, f64),
    ) {
        let p = ui.painter();
        card(p, r, look);
        let s = &e.stats;
        let variant = variant_of(s);
        let (badge, lang_color) = mode_badge(s.language, variant);
        let bx = Rect::from_min_size(r.min + vec2(18.0, 18.0), vec2(86.0, r.height() - 36.0));
        cover(ui, look, bx, tex, &e.title, lang_color);

        let left = bx.right() + 20.0;
        let width = r.right() - 20.0 - left;
        let top = r.top() + 18.0;
        let g = galley(p, &e.title, title_font(19.0), look.text, width, 2);
        let title_h = g.size().y;
        p.galley(pos2(left, top), g, look.text);

        // Language, difficulty, read, writing style.
        let y = top + title_h + 16.0;
        let mut x = left;
        x += pill(p, pos2(x, y), badge, 11.5, palette::cat().crust, lang_color) + 6.0;
        if let Some(k) = &s.known {
            let counted = k.running_words - self.style_offset(&e.id);
            let d = Difficulty::from_percent(counted);
            let c = difficulty_color(d);
            x += pill(
                p,
                pos2(x, y),
                &format!("{} · {:.1}% known", d.label(), k.running_words),
                11.5,
                c,
                mix(c, look.card, 0.16),
            ) + 6.0;
        }
        if read.is_some() {
            let c = palette::good();
            x += pill(p, pos2(x, y), "✔ Read", 11.5, c, mix(c, look.card, 0.16)) + 6.0;
        }
        if let Some(st) = style
            && x + 80.0 < r.right()
        {
            pill(p, pos2(x, y), &st, 11.5, look.weak, look.faint);
        }

        // Figures along the bottom.
        let figures: Vec<(&str, String)> = if s.language == Language::Korean {
            vec![
                ("Words", fmt_int(s.words_total)),
                ("Unique", fmt_int(s.words_unique)),
                ("For 98%", fmt_int(s.words_for_98)),
                ("Reading", fmt_time(reading_minutes(s, speeds))),
            ]
        } else {
            vec![
                (
                    if s.language == Language::Japanese {
                        "Kanji"
                    } else {
                        "Characters"
                    },
                    fmt_int(s.han_total),
                ),
                ("Words", fmt_int(s.words_total)),
                ("Unique words", fmt_int(s.words_unique)),
                ("Reading", fmt_time(reading_minutes(s, speeds))),
            ]
        };
        let col = width / figures.len() as f32;
        let bottom = r.bottom() - 18.0;
        for (i, (label, value)) in figures.iter().enumerate() {
            let fx = left + i as f32 * col;
            p.text(
                pos2(fx, bottom),
                Align2::LEFT_BOTTOM,
                *label,
                regular(11.5),
                look.weak,
            );
            p.text(
                pos2(fx, bottom - 16.0),
                Align2::LEFT_BOTTOM,
                value,
                bold(17.0),
                look.text,
            );
        }
    }

    fn book_page(&self, ui: &mut egui::Ui, look: Look, covers: &Covers) {
        let Some(r) = self.report.as_ref() else {
            return;
        };
        let lang = r.language;
        let chinese = lang == Language::Chinese;
        let japanese = lang == Language::Japanese;
        let title = r
            .title
            .clone()
            .or_else(|| self.path.as_deref().map(crate::file_stem))
            .unwrap_or_default();
        let (badge, lang_color) = mode_badge(lang, chinese.then_some(r.variant));
        let style_off = self
            .report_id
            .as_deref()
            .map_or(0.0, |id| self.style_offset(id));
        let style_rating = (chinese && self.style_on())
            .then(|| {
                let id = self.report_id.as_deref()?;
                let s = self.style.store.get(id)?;
                Some(format!(
                    "Writing style {} · {}",
                    self.style_scale().pct(s.score),
                    s.label()
                ))
            })
            .flatten();
        let (included, sections) = (
            self.included.iter().filter(|x| **x).count(),
            self.included.len(),
        );
        // Reading speed as set on the Overview.
        let (minutes, speed) = match lang {
            Language::Chinese => {
                let v = self.cpm.unwrap_or(self.settings.cpm) as f64;
                (
                    r.han_total as f64 / v,
                    format!("{v:.0} characters a minute"),
                )
            }
            Language::Japanese => {
                let v = self.ja_cpm.unwrap_or(self.settings.ja_cpm) as f64;
                (
                    (r.han_total + r.kana_total) as f64 / v,
                    format!("{v:.0} characters a minute"),
                )
            }
            Language::Korean => {
                let v = self.wpm.unwrap_or(self.settings.wpm) as f64;
                (r.words_total as f64 / v, format!("{v:.0} words a minute"))
            }
        };
        let cover_at = |cov: &[hanstat::analyze::Coverage], p: f64| {
            cov.iter()
                .find(|c| c.percent == p)
                .map_or(0, |c| c.items as u64)
        };

        page(ui, look, |ui| {
            // The cover and the title.
            frame(look).inner_margin(28).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    let (bx, _) = ui.allocate_exact_size(vec2(170.0, 240.0), Sense::hover());
                    cover(ui, look, bx, covers.get(""), &title, lang_color);
                    ui.add_space(14.0);
                    ui.vertical(|ui| {
                        let (r0, _) = ui
                            .allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
                        caps(
                            ui.painter(),
                            r0.left_top(),
                            "hanstat · Book report",
                            palette::accent(),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(&title)
                                .size(38.0)
                                .family(FontFamily::Name(TITLE.into()))
                                .color(look.text),
                        );
                        ui.add_space(12.0);
                        let mut items = vec![(badge.to_owned(), palette::cat().crust, lang_color)];
                        items.push((r.format.to_owned(), look.weak, look.faint));
                        items.push((r.segmenter.label().to_owned(), look.weak, look.faint));
                        items.push((
                            format!("{included} of {sections} sections"),
                            look.weak,
                            look.faint,
                        ));
                        pills(ui, &items, 12.5);
                        ui.add_space(18.0);
                        if let Some(c) = &r.anki {
                            let p_known = c.running_words.percent();
                            let counted = p_known - style_off;
                            let d = Difficulty::from_percent(counted);
                            ui.label(bold_text(d.label(), 30.0, difficulty_color(d)));
                            ui.add_space(4.0);
                            let mut sub = format!(
                                "{} · {p_known:.1}% of running words known",
                                d.description()
                            );
                            if let Some(s) = &style_rating {
                                sub.push_str(&format!(" · {s}"));
                            }
                            ui.label(RichText::new(sub).size(14.5).color(look.weak));
                            ui.add_space(6.0);
                            gauge(ui, look, p_known, counted);
                        } else if let Some(s) = &style_rating {
                            ui.label(RichText::new(s).size(15.0).color(look.weak));
                        }
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!(
                                "About {} of reading at {speed}",
                                fmt_time(minutes)
                            ))
                            .size(15.0)
                            .color(look.text),
                        );
                    });
                });
            });

            // The text.
            section(ui, look, "The text", "");
            let mut t = Vec::new();
            if chinese || japanese {
                t.push(tile(
                    if japanese { "Kanji" } else { "Characters" },
                    fmt_int(r.han_total),
                    format!(
                        "{} unique · {} once",
                        fmt_int(r.han_unique as u64),
                        fmt_int(r.han_hapax as u64)
                    ),
                ));
            }
            if japanese {
                t.push(tile(
                    "Kana",
                    fmt_int(r.kana_total),
                    "counted with the kanji for reading time",
                ));
            }
            t.push(tile(
                "Words",
                fmt_int(r.words_total),
                format!(
                    "{} unique · {} once",
                    fmt_int(r.words_unique as u64),
                    fmt_int(r.words_hapax as u64)
                ),
            ));
            if chinese || japanese {
                t.push(tile(
                    if japanese {
                        "Kanji for 99%"
                    } else {
                        "Characters for 99%"
                    },
                    fmt_int(cover_at(&r.char_coverage, 99.0)),
                    format!("{} for 95%", fmt_int(cover_at(&r.char_coverage, 95.0))),
                ));
            }
            t.push(tile(
                "Words for 98%",
                fmt_int(cover_at(&r.word_coverage, 98.0)),
                format!("{} for 95%", fmt_int(cover_at(&r.word_coverage, 95.0))),
            ));
            t.push(tile(
                "Reading time",
                fmt_time(minutes),
                format!("at {speed}"),
            ));
            t.push(tile(
                "Sentences",
                fmt_int(r.sentences),
                if chinese {
                    format!("{:.1} characters each", r.avg_sentence_han)
                } else {
                    format!("{:.1} words each", r.avg_sentence_words.unwrap_or(0.0))
                },
            ));
            t.push(tile(
                "Paragraphs",
                fmt_int(r.paragraphs),
                if chinese {
                    format!("{:.1} characters each", r.avg_paragraph_han)
                } else {
                    format!("{:.1} words each", r.avg_paragraph_words.unwrap_or(0.0))
                },
            ));
            if chinese {
                t.push(tile(
                    "Average word",
                    format!("{:.2}", r.avg_word_len),
                    "characters long",
                ));
            }
            tiles(ui, look, cols_for(t.len()), &t);

            // What is known.
            if let Some(c) = &r.anki {
                let lists = &c.listed.source;
                let source = match (c.from_anki, lists.is_empty()) {
                    (true, true) => "Anki".to_owned(),
                    (true, false) => format!("Anki + {lists}"),
                    (false, _) => lists.clone(),
                };
                section(
                    ui,
                    look,
                    "What you know",
                    &format!("Known from {source}; names are left out"),
                );
                let rw = &c.running_words;
                let unknown = rw.not_known();
                let d = Difficulty::from_percent(rw.percent() - style_off);
                let mut t = vec![
                    tile(
                        "Running words known",
                        format!("{:.1}%", rw.percent()),
                        if unknown > 0 {
                            format!(
                                "1 unknown every {:.1} words",
                                rw.total as f64 / unknown as f64
                            )
                        } else {
                            "no unknown words".to_owned()
                        },
                    )
                    .color(difficulty_color(d)),
                    tile(
                        "Unique words known",
                        format!("{:.1}%", c.unique_words.percent()),
                        format!(
                            "{} of {} · {} not known",
                            fmt_int(c.unique_words.known),
                            fmt_int(c.unique_words.total),
                            fmt_int(c.unique_words.not_known())
                        ),
                    ),
                ];
                if chinese || japanese {
                    t.push(tile(
                        if japanese {
                            "Kanji known"
                        } else {
                            "Characters known"
                        },
                        format!("{:.1}%", c.unique_chars.percent()),
                        format!(
                            "{} of {}",
                            fmt_int(c.unique_chars.known),
                            fmt_int(c.unique_chars.total)
                        ),
                    ));
                }
                t.push(tile("Difficulty", d.label(), d.description()).color(difficulty_color(d)));
                tiles(ui, look, cols_for(t.len()), &t);
            }

            // Coverage: the curve and the table.
            ui.add_space(GAP);
            ui.columns(2, |cols| {
                let counts: Vec<u32> = r.word_freq.iter().map(|x| x.1).collect();
                let pts = coverage_points(&counts);
                let marks: Vec<(f64, usize)> = [95.0, 98.0]
                    .into_iter()
                    .map(|p| (p, cover_at(&r.word_coverage, p) as usize))
                    .filter(|x| x.1 > 0)
                    .collect();
                let level = r.anki.as_ref().map(|c| {
                    let p = c.running_words.percent();
                    (p, format!("You know {p:.1}% of the running words"))
                });
                // The table first: the curve fills a card as tall as it.
                let table = chart_card(
                    &mut cols[1],
                    look,
                    "Words to know",
                    "The most frequent items needed to recognise a share of the running text",
                    |ui| coverage_table(ui, look, r, chinese || japanese, japanese),
                );
                sized_card(
                    &mut cols[0],
                    look,
                    "Vocabulary coverage",
                    "How much of the text its most frequent words make up (log scale)",
                    Some(table),
                    |ui, height| {
                        charts::coverage_curve(
                            ui,
                            &pts,
                            &marks,
                            level,
                            palette::accent(),
                            "words",
                            height,
                        )
                    },
                );
            });
        });
    }
}

/// Rows of coverage: percent, characters (if any), words.
fn coverage_table(
    ui: &mut egui::Ui,
    look: Look,
    r: &hanstat::analyze::Report,
    chars: bool,
    japanese: bool,
) {
    let rows = r.word_coverage.len();
    let row_h = 28.0;
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), row_h * (rows + 1) as f32),
        Sense::hover(),
    );
    let p = ui.painter();
    let cols: Vec<f32> = if chars {
        vec![
            rect.left() + 8.0,
            rect.left() + rect.width() * 0.55,
            rect.right() - 8.0,
        ]
    } else {
        vec![rect.left() + 8.0, rect.right() - 8.0]
    };
    let head_y = rect.top() + row_h / 2.0;
    p.text(
        pos2(cols[0], head_y),
        Align2::LEFT_CENTER,
        "COVERAGE",
        bold(11.0),
        look.weak,
    );
    if chars {
        p.text(
            pos2(cols[1], head_y),
            Align2::RIGHT_CENTER,
            if japanese { "KANJI" } else { "CHARACTERS" },
            bold(11.0),
            look.weak,
        );
    }
    p.text(
        pos2(*cols.last().unwrap(), head_y),
        Align2::RIGHT_CENTER,
        "WORDS",
        bold(11.0),
        look.weak,
    );
    for (i, w) in r.word_coverage.iter().enumerate() {
        let top = rect.top() + row_h * (i + 1) as f32;
        let row = Rect::from_min_size(pos2(rect.left(), top), vec2(rect.width(), row_h));
        if i % 2 == 0 {
            p.rect_filled(row, 6.0, look.faint);
        }
        let y = row.center().y;
        p.text(
            pos2(cols[0], y),
            Align2::LEFT_CENTER,
            format!("{}%", w.percent),
            bold(14.0),
            look.text,
        );
        if chars {
            let c = r
                .char_coverage
                .get(i)
                .map_or("–".into(), |c| fmt_int(c.items as u64));
            p.text(
                pos2(cols[1], y),
                Align2::RIGHT_CENTER,
                c,
                regular(14.0),
                look.text,
            );
        }
        p.text(
            pos2(*cols.last().unwrap(), y),
            Align2::RIGHT_CENTER,
            fmt_int(w.items as u64),
            regular(14.0),
            look.text,
        );
    }
}
