//! The EPUB reader: a paginated view of a Chinese book with its own layout,
//! styles and images, in a Solarized Light page.
//!
//! Chapters are parsed and styled on a background thread (the open one
//! first); the open chapter is laid out on the UI thread when it is shown,
//! with the fonts chosen in the reader, and drawn page by page.

mod css;
mod epub;
pub(crate) mod fonts;
mod images;
mod layout;
mod markup;
#[cfg(test)]
mod preview;
mod style;
mod tree;

use eframe::egui::{self, Color32, Key, Pos2, Rect, RichText, Vec2, pos2, vec2};
use epub::Epub;
use layout::{ChapterLayout, Item, Params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use style::LineStyle;
use tree::Chapter;

/// Solarized (Ethan Schoonover), light.
mod theme {
    use eframe::egui::Color32;
    pub const BASE01: Color32 = Color32::from_rgb(0x58, 0x6e, 0x75);
    pub const BASE00: Color32 = Color32::from_rgb(0x65, 0x7b, 0x83);
    pub const BASE1: Color32 = Color32::from_rgb(0x93, 0xa1, 0xa1);
    pub const BASE2: Color32 = Color32::from_rgb(0xee, 0xe8, 0xd5);
    pub const BASE3: Color32 = Color32::from_rgb(0xfd, 0xf6, 0xe3);
    pub const BLUE: Color32 = Color32::from_rgb(0x26, 0x8b, 0xd2);
    /// Body text: between base02 and base01, darker than Solarized's own
    /// base00 so thin serif strokes stay crisp.
    pub const INK: Color32 = Color32::from_rgb(0x1f, 0x3c, 0x46);
    /// The desk around the pages.
    pub const DESK: Color32 = Color32::from_rgb(0xe4, 0xdd, 0xc7);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Position {
    spine: usize,
    offset: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    /// Chosen families; `None`: the default for the book's script.
    serif: Option<String>,
    sans: Option<String>,
    /// Text whose style names no family is set in sans-serif.
    sans_default: bool,
    /// Text size in points.
    size: f32,
    /// Lines at least this many times the text size apart.
    line_spacing: f32,
    /// Page margins, as a factor of the default.
    margins: f32,
    /// Two pages side by side when the window is wide enough.
    spread: bool,
    /// Where each book was left, by its Library id.
    positions: HashMap<String, Position>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            serif: None,
            sans: None,
            sans_default: false,
            size: 20.0,
            line_spacing: 1.75,
            margins: 1.0,
            spread: true,
            positions: HashMap::new(),
        }
    }
}

impl Settings {
    fn path() -> std::path::PathBuf {
        hanstat::models::app_dir().join("reader.json")
    }
    fn load() -> Settings {
        std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    fn save(&self) -> Result<(), String> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, serde_json::to_vec_pretty(self).unwrap())
            .map_err(|e| format!("Cannot save {}: {e}", p.display()))
    }
}

/// Widgets of the settings panel as drawn in the last frame.
#[derive(Default)]
struct Probe {
    aa: Option<Rect>,
    /// The serif font box, while the panel is open.
    serif_box: Option<Rect>,
    /// The serif list is open: one of its entries and its family.
    serif_item: Option<(Rect, String)>,
    contents: Option<Rect>,
    /// The contents menu is open: its sixth entry.
    toc_item: Option<Rect>,
}

/// Where to go once a chapter is laid out.
#[derive(Clone, Debug)]
enum Target {
    Offset(usize),
    Anchor(String),
    Last,
}

/// Page geometry for a window size.
#[derive(Clone, Copy, PartialEq)]
struct Geometry {
    /// The pages' outer rectangles.
    pages: [Rect; 2],
    spread: bool,
    /// Text area offset inside a page.
    inset: Vec2,
    content: Vec2,
}

pub struct Reader {
    epub: Arc<Epub>,
    book_id: String,
    traditional: bool,
    settings: Settings,
    /// Chapters built by the worker, by spine index.
    chapters: Arc<Mutex<Vec<Option<Arc<Chapter>>>>>,
    want: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    layouts: HashMap<usize, Arc<ChapterLayout>>,
    params: Option<Params>,
    images: images::Images,
    fonts: Option<fonts::Installed>,
    generation: u64,
    spine: usize,
    page: usize,
    target: Option<Target>,
    /// Direction of the last move (skipping empty chapters).
    forward: bool,
    offset: usize,
    prev_style: Option<(egui::Theme, Arc<egui::Style>)>,
    scroll: f32,
    resized_at: f64,
    last_geom: Option<Geometry>,
    /// The geometry the laid-out pages were made for (kept while the
    /// window is resized or a slider dragged).
    drawn: Option<Geometry>,
    /// Settings changed and not yet saved; `persist`: save at all.
    dirty: bool,
    persist: bool,
    /// Table of contents entries as shown: numbered when the book repeats names.
    toc_labels: Vec<String>,
    /// The contents menu was open last frame.
    contents_shown: bool,
    /// The Aa panel is open.
    settings_open: bool,
    /// Where the settings' widgets were drawn this frame (for tests that click them).
    probe: Probe,
    error: Option<String>,
    /// The time of this frame, and of the last page turn.
    now: f64,
    turned_at: f64,
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Reader {
    pub fn open(
        path: &Path,
        book_id: Option<&str>,
        traditional: bool,
        ctx: &egui::Context,
    ) -> Result<Reader, String> {
        let epub = Arc::new(Epub::open(path)?);
        let settings = Settings::load();
        let book_id = book_id
            .map(str::to_owned)
            .unwrap_or_else(|| path.display().to_string());
        let pos = settings
            .positions
            .get(&book_id)
            .copied()
            .unwrap_or_default();
        let spine = pos.spine.min(epub.spine.len() - 1);
        let n = epub.spine.len();
        let chapters = Arc::new(Mutex::new(vec![None; n]));
        let want = Arc::new(AtomicUsize::new(spine));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (epub, chapters, want, stop, ctx) = (
                epub.clone(),
                chapters.clone(),
                want.clone(),
                stop.clone(),
                ctx.clone(),
            );
            std::thread::Builder::new()
                .name("reader-chapters".into())
                .spawn(move || {
                    let mut sheets = tree::SheetCache::default();
                    loop {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        let cur = want.load(Ordering::Relaxed) as isize;
                        let next = {
                            let c = chapters.lock().unwrap();
                            (0..c.len())
                                .filter(|i| c[*i].is_none())
                                // Ahead before behind.
                                .min_by_key(|i| {
                                    (
                                        (*i as isize - cur).abs() * 2
                                            + (*i < cur as usize) as isize,
                                        *i,
                                    )
                                })
                        };
                        let Some(i) = next else { return };
                        let ch = tree::build(&epub, &epub.spine[i], &mut sheets);
                        chapters.lock().unwrap()[i] = Some(Arc::new(ch));
                        if (i as isize - cur).abs() <= 1 {
                            ctx.request_repaint();
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        let toc_labels = toc_labels(&epub.toc);
        fonts::prefetch();
        // The reader's colours for every widget and popup while it is open.
        let theme = ctx.theme();
        let prev = ctx.style_of(theme);
        let mut style = (*prev).clone();
        style.visuals = visuals();
        ctx.set_style_of(theme, style);
        Ok(Reader {
            images: images::Images::new(epub.clone()),
            epub,
            book_id,
            traditional,
            settings,
            chapters,
            want,
            stop,
            layouts: HashMap::new(),
            params: None,
            fonts: None,
            generation: 0,
            spine,
            page: 0,
            target: Some(Target::Offset(pos.offset)),
            forward: true,
            offset: pos.offset,
            prev_style: Some((theme, prev)),
            scroll: 0.0,
            resized_at: 0.0,
            last_geom: None,
            drawn: None,
            dirty: false,
            persist: true,
            toc_labels,
            contents_shown: false,
            settings_open: false,
            probe: Probe::default(),
            error: None,
            now: 0.0,
            turned_at: 0.0,
        })
    }

    /// Restore the app's look and remember the place.
    pub fn close(mut self, ctx: &egui::Context) {
        self.save();
        if let Some((theme, style)) = self.prev_style.take() {
            ctx.set_style_of(theme, style);
        }
    }

    pub fn save(&mut self) {
        self.dirty = false;
        if !self.persist {
            return;
        }
        self.settings.positions.insert(
            self.book_id.clone(),
            Position {
                spine: self.spine,
                offset: self.offset,
            },
        );
        if let Err(e) = self.settings.save() {
            self.error = Some(e);
        }
    }

    fn chapter(&self, i: usize) -> Option<Arc<Chapter>> {
        self.chapters.lock().unwrap().get(i).cloned().flatten()
    }

    /// Fonts as chosen; reinstalled when the choice changes. `None` until the
    /// font scan is done and egui has the families.
    fn fonts_ready(&mut self, ctx: &egui::Context) -> Option<fonts::Installed> {
        let fams = fonts::families()?;
        let serif = self
            .settings
            .serif
            .clone()
            .unwrap_or_else(|| fonts::default_choice(&fams, true, self.traditional));
        let sans = self
            .settings
            .sans
            .clone()
            .unwrap_or_else(|| fonts::default_choice(&fams, false, self.traditional));
        if self
            .fonts
            .as_ref()
            .is_none_or(|f| f.key != (serif.clone(), sans.clone()))
        {
            self.generation += 1;
            self.fonts = Some(fonts::install(ctx, &fams, &serif, &sans, self.generation));
            self.layouts.clear();
        }
        let f = self.fonts.clone()?;
        let active = ctx.fonts(|v| v.definitions().families.contains_key(&f.serif));
        if !active {
            ctx.request_repaint();
            return None;
        }
        Some(f)
    }

    fn geometry(&self, area: Rect) -> Geometry {
        geometry(&self.settings, area)
    }

    fn step(&self, g: &Geometry) -> usize {
        if g.spread { 2 } else { 1 }
    }

    /// The layout of chapter `i`, made now if its tree is ready.
    fn layout(&mut self, ctx: &egui::Context, i: usize) -> Option<Arc<ChapterLayout>> {
        if let Some(l) = self.layouts.get(&i) {
            return Some(l.clone());
        }
        let ch = self.chapter(i)?;
        let params = self.params.clone()?;
        let images = &mut self.images;
        let l = ctx.fonts_mut(|f| layout::layout(&ch, &params, f, images));
        let l = Arc::new(l);
        self.layouts.insert(i, l.clone());
        // Keep the open chapter and its neighbours.
        let keep = self.spine;
        self.layouts
            .retain(|k, _| (*k as isize - keep as isize).abs() <= 2);
        Some(l)
    }

    fn go_chapter(&mut self, i: usize, target: Target) {
        self.turned_at = self.now;
        self.spine = i.min(self.epub.spine.len() - 1);
        self.want.store(self.spine, Ordering::Relaxed);
        self.target = Some(target);
    }

    fn next(&mut self, g: &Geometry) {
        self.forward = true;
        self.turned_at = self.now;
        let step = self.step(g);
        if let Some(l) = self.layouts.get(&self.spine)
            && self.page + step < l.pages.len()
        {
            self.page += step;
            self.offset = l.page_offset(self.page);
            return;
        }
        if self.spine + 1 < self.epub.spine.len() {
            self.go_chapter(self.spine + 1, Target::Offset(0));
            self.save();
        }
    }

    fn prev(&mut self, g: &Geometry) {
        self.forward = false;
        self.turned_at = self.now;
        let step = self.step(g);
        if self.page >= step {
            self.page -= step;
            if let Some(l) = self.layouts.get(&self.spine) {
                self.offset = l.page_offset(self.page);
            }
            return;
        }
        if self.spine > 0 {
            self.go_chapter(self.spine - 1, Target::Last);
        }
    }

    fn follow(&mut self, path: &str, fragment: Option<String>) {
        if let Some(i) = self.epub.spine.iter().position(|p| p == path) {
            self.forward = true;
            self.go_chapter(i, fragment.map_or(Target::Offset(0), Target::Anchor));
            self.save();
        }
    }

    /// The table of contents entry the reader is in.
    fn toc_label(&self) -> Option<&str> {
        self.toc_current().map(|i| self.toc_labels[i].as_str())
    }

    /// The index of the table of contents entry the reader is in.
    fn toc_current(&self) -> Option<usize> {
        let mut best = None;
        for (i, e) in self.epub.toc.iter().enumerate() {
            if e.spine > self.spine {
                break;
            }
            if e.spine == self.spine
                && let Some(f) = &e.fragment
                && let Some(l) = self.layouts.get(&self.spine)
                && l.page_of_anchor(f).is_some_and(|p| p > self.page)
            {
                break;
            }
            best = Some(i);
        }
        best
    }

    /// Each contents entry's length in characters (letters, digits and
    /// Chinese characters): from where it starts to where the next one does.
    /// `None` until every chapter is built.
    fn toc_lengths(&self) -> Option<Vec<usize>> {
        let c = self.chapters.lock().unwrap();
        let mut before = Vec::with_capacity(c.len() + 1);
        let mut total = 0usize;
        for ch in c.iter() {
            before.push(total);
            total += ch.as_ref()?.text_len;
        }
        let at = |e: &epub::TocEntry| -> usize {
            let ch = c[e.spine].as_ref().unwrap();
            let within = e
                .fragment
                .as_ref()
                .and_then(|f| ch.anchors.iter().find(|(id, _)| id == f))
                .map_or(0, |(_, n)| *n);
            before[e.spine] + within
        };
        let starts: Vec<usize> = self.epub.toc.iter().map(at).collect();
        Some(
            (0..starts.len())
                .map(|i| {
                    starts
                        .get(i + 1)
                        .copied()
                        .unwrap_or(total)
                        .saturating_sub(starts[i])
                })
                .collect(),
        )
    }

    /// The share of the book before the current page, once every chapter is read.
    fn progress(&self) -> Option<f32> {
        let c = self.chapters.lock().unwrap();
        let mut total = 0usize;
        let mut before = 0usize;
        for (i, ch) in c.iter().enumerate() {
            let n = ch.as_ref()?.text_len;
            total += n;
            if i < self.spine {
                before += n;
            }
        }
        let here = c[self.spine]
            .as_ref()
            .map_or(0, |ch| self.offset.min(ch.text_len));
        (total > 0).then(|| (before + here) as f32 / total as f32)
    }

    /// Draw the reader. `false` once it is closed.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        self.now = now;
        self.images.next_frame();
        self.images.poll(&ctx);
        let mut open = true;
        // A click that closes a popup doesn't also turn the page.
        let popup_was_open = egui::Popup::is_any_open(&ctx) || self.settings_open;
        // Esc closes an open panel or menu (egui closes its menus itself
        // while drawing them), never the reader with it.
        let esc_for_popup = popup_was_open && ctx.input(|i| i.key_pressed(Key::Escape));
        if esc_for_popup {
            self.settings_open = false;
        }
        let mut geom_for_keys: Option<Geometry> = self.last_geom;

        egui::Panel::top("reader-bar")
            .frame(
                egui::Frame::new()
                    .fill(theme::BASE2)
                    .inner_margin(egui::Margin::symmetric(10, 6)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button(RichText::new("‹ Back").size(15.0))
                        .on_hover_text("Back to the statistics (Esc)")
                        .clicked()
                    {
                        open = false;
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(&self.epub.title)
                            .size(15.0)
                            .color(theme::BASE01)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.settings_menu(ui);
                        self.contents_menu(ui);
                        ui.separator();
                        let before = self.settings.spread;
                        ui.selectable_value(
                            &mut self.settings.spread,
                            true,
                            RichText::new("Two pages").size(14.0),
                        )
                        .on_hover_text("Two pages side by side, when the window is wide enough");
                        ui.selectable_value(
                            &mut self.settings.spread,
                            false,
                            RichText::new("One page").size(14.0),
                        );
                        if self.settings.spread != before {
                            self.dirty = true;
                        }
                        ui.separator();
                        if let Some(p) = self.progress() {
                            ui.label(
                                RichText::new(format!("{:.0}%", p * 100.0)).color(theme::BASE1),
                            )
                            .on_hover_text("Of the book's text, before this page");
                        }
                    });
                });
            });

        egui::CentralPanel::no_frame().show(ui, |ui| {
            let area = ui.max_rect();
            ui.painter().rect_filled(area, 0.0, theme::DESK);
            let g = self.geometry(area);
            geom_for_keys = Some(g);
            // While the window is being resized, the pages laid out before stay
            // until it settles.
            if self.last_geom.is_some_and(|l| l.content != g.content) {
                self.resized_at = now;
            }
            self.last_geom = Some(g);
            let Some(fonts) = self.fonts_ready(&ctx) else {
                centered_note(ui, area, "Loading fonts…");
                return;
            };
            let params = Params {
                base: self.settings.size,
                content_w: g.content.x,
                page_h: g.content.y,
                line_spacing: self.settings.line_spacing,
                fonts,
                serif_default: !self.settings.sans_default,
                text: theme::INK,
                link: theme::BLUE,
                rule: theme::BASE1,
            };
            // Not while a slider or the window edge is being dragged.
            let settled = (now - self.resized_at > 0.15 && !ctx.input(|i| i.pointer.any_down()))
                || self.layouts.is_empty();
            if self.params.as_ref() != Some(&params) {
                if settled {
                    self.params = Some(params);
                    self.layouts.clear();
                    self.drawn = Some(g);
                    self.target.get_or_insert(Target::Offset(self.offset));
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(160));
                }
            }
            let g = self.drawn.unwrap_or(g);
            geom_for_keys = Some(g);
            let Some(l) = self.layout(&ctx, self.spine) else {
                centered_note(ui, area, "Opening…");
                return;
            };
            let step = self.step(&g);
            if let Some(t) = self.target.take() {
                if l.pages.is_empty() {
                    // Nothing to show here: skip in the direction of travel.
                    let n = self.epub.spine.len();
                    if self.forward && self.spine + 1 < n {
                        self.go_chapter(self.spine + 1, Target::Offset(0));
                    } else if !self.forward && self.spine > 0 {
                        self.go_chapter(self.spine - 1, Target::Last);
                    }
                    ctx.request_repaint();
                    return;
                }
                let by_offset = matches!(t, Target::Offset(_));
                self.page = match t {
                    Target::Offset(o) => l.page_of_offset(o),
                    Target::Anchor(a) => l.page_of_anchor(&a).unwrap_or(0),
                    Target::Last => l.pages.len() - 1,
                };
                self.page -= self.page % step;
                if !by_offset {
                    self.offset = l.page_offset(self.page);
                }
            }
            self.page = self.page.min(l.pages.len().saturating_sub(1));
            self.page -= self.page % step;
            let label = self.toc_label().map(str::to_owned);
            let resp = ui.interact(area, egui::Id::unique("reader-page"), egui::Sense::click());
            let mut clicked_link = None;
            for side in 0..step {
                let page_rect = g.pages[side];
                draw_page(ui, page_rect, side, g.spread);
                let idx = self.page + side;
                if idx >= l.pages.len() {
                    continue;
                }
                let origin = page_rect.min + g.inset;
                let mut off = origin.to_vec2();
                if l.pages[idx].center {
                    off.y += ((g.content.y - l.page_height(idx)) / 2.0).max(0.0);
                }
                let (items, shift) = l.page_items(idx);
                let off = off + shift;
                let clip = Rect::from_min_size(origin, g.content).expand(4.0);
                let painter = ui.painter_at(clip.expand2(vec2(g.inset.x * 0.5, 0.0)));
                let ppp = ctx.pixels_per_point();
                for i in items {
                    paint_item(&mut self.images, &painter, &l.items[i], off, ppp);
                }
                // Running head and folio.
                let small = egui::FontId::proportional((self.settings.size * 0.62).max(11.0));
                if let Some(t) = &label {
                    painter_free(ui).text(
                        pos2(page_rect.center().x, page_rect.top() + g.inset.y * 0.45),
                        egui::Align2::CENTER_CENTER,
                        t,
                        small.clone(),
                        theme::BASE1,
                    );
                }
                painter_free(ui).text(
                    pos2(page_rect.center().x, page_rect.bottom() - g.inset.y * 0.5),
                    egui::Align2::CENTER_CENTER,
                    format!("{} / {}", idx + 1, l.pages.len()),
                    small,
                    theme::BASE1,
                );
                if let Some(p) = resp.interact_pointer_pos()
                    && resp.clicked()
                {
                    for link in l.page_links(idx) {
                        if link.rect.translate(off).contains(p) {
                            clicked_link = Some(link.target);
                        }
                    }
                }
                if l.page_links(idx).any(|k| {
                    resp.hover_pos()
                        .is_some_and(|h| k.rect.translate(off).contains(h))
                }) {
                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
            // Clicks: links, else the left third back and the rest forward.
            if resp.clicked() && popup_was_open {
                self.settings_open = false;
            } else if resp.clicked() {
                if let Some(t) = clicked_link {
                    if let Some(ch) = self.chapter(self.spine)
                        && let Some((path, frag)) = ch.links.get(t as usize).cloned()
                    {
                        self.follow(&path, frag);
                    }
                } else if let Some(p) = resp.interact_pointer_pos() {
                    let left = g.pages[0].left();
                    let right = if g.spread {
                        g.pages[1].right()
                    } else {
                        g.pages[0].right()
                    };
                    if p.x < left + (right - left) * 0.3 {
                        self.prev(&g);
                    } else {
                        self.next(&g);
                    }
                }
            }
            // Lay out the next chapter once the reader has been idle a moment,
            // so turning into it is instant and the work never delays a turn.
            let idle = now - self.turned_at > 0.5;
            if self.target.is_none()
                && self.spine + 1 < self.epub.spine.len()
                && !self.layouts.contains_key(&(self.spine + 1))
            {
                if idle {
                    let _ = self.layout(&ctx, self.spine + 1);
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(550));
                }
            }
        });

        // Keys and the wheel turn pages.
        if let Some(g) = geom_for_keys
            && !ctx.text_edit_focused()
            && !egui::Popup::is_any_open(&ctx)
            && !self.settings_open
            && !popup_was_open
        {
            let (fwd, back, home, end, esc, wheel) = ctx.input_mut(|i| {
                let fwd = [Key::ArrowRight, Key::ArrowDown, Key::PageDown, Key::Space]
                    .iter()
                    .any(|k| i.consume_key(egui::Modifiers::NONE, *k));
                let back = [Key::ArrowLeft, Key::ArrowUp, Key::PageUp, Key::Backspace]
                    .iter()
                    .any(|k| i.consume_key(egui::Modifiers::NONE, *k))
                    || i.consume_key(egui::Modifiers::SHIFT, Key::Space);
                (
                    fwd,
                    back,
                    i.consume_key(egui::Modifiers::NONE, Key::Home),
                    i.consume_key(egui::Modifiers::NONE, Key::End),
                    i.consume_key(egui::Modifiers::NONE, Key::Escape),
                    i.raw
                        .events
                        .iter()
                        .map(|e| match e {
                            egui::Event::MouseWheel { unit, delta, .. } => match unit {
                                egui::MouseWheelUnit::Point => delta.y,
                                egui::MouseWheelUnit::Line => delta.y * 40.0,
                                egui::MouseWheelUnit::Page => delta.y * 400.0,
                            },
                            _ => 0.0,
                        })
                        .sum::<f32>(),
                )
            });
            if fwd {
                self.next(&g);
            }
            if back {
                self.prev(&g);
            }
            if home {
                self.page = 0;
                self.offset = 0;
            }
            if end && let Some(l) = self.layouts.get(&self.spine) {
                self.page = l.pages.len().saturating_sub(1);
                self.offset = l.page_offset(self.page);
            }
            if esc {
                open = false;
            }
            self.scroll += wheel;
            if self.scroll.abs() >= 60.0 {
                if self.scroll < 0.0 {
                    self.next(&g);
                } else {
                    self.prev(&g);
                }
                self.scroll = 0.0;
            } else if wheel == 0.0 {
                self.scroll *= 0.8;
            }
        }
        if self.dirty && !ctx.input(|i| i.pointer.any_down()) {
            self.save();
        }
        if let Some(e) = self.error.take() {
            egui::Window::new("Reader")
                .collapsible(false)
                .show(&ctx, |ui| ui.label(e));
        }
        open
    }

    fn contents_menu(&mut self, ui: &mut egui::Ui) {
        if self.epub.toc.is_empty() {
            return;
        }
        let mut go = None;
        let current = self.toc_current();
        let lengths = self.toc_lengths();
        let scroll = !self.contents_shown;
        self.probe.toc_item = None;
        let mut toc_item = None;
        // Each entry is one line of text: its name, then its length in a
        // column just past the longest name, so the menu is as wide as its
        // contents (a right-aligned column would stretch it across the window).
        let font = egui::FontId::proportional(15.0);
        let small = egui::FontId::proportional(13.0);
        let indent = |e: &epub::TocEntry| e.depth as f32 * 16.0;
        let name_w: Vec<f32> = ui.fonts_mut(|f| {
            self.toc_labels
                .iter()
                .map(|l| {
                    f.layout_no_wrap(l.clone(), font.clone(), Color32::WHITE)
                        .size()
                        .x
                })
                .collect()
        });
        let numbers: Vec<String> = (0..self.toc_labels.len())
            .map(|i| {
                lengths
                    .as_ref()
                    .map_or("…".to_owned(), |l| group_digits(l[i]))
            })
            .collect();
        let number_w: Vec<f32> = ui.fonts_mut(|f| {
            numbers
                .iter()
                .map(|n| {
                    f.layout_no_wrap(n.clone(), small.clone(), Color32::WHITE)
                        .size()
                        .x
                })
                .collect()
        });
        let names_col = self
            .epub
            .toc
            .iter()
            .zip(&name_w)
            .map(|(e, w)| indent(e) + w)
            .fold(0.0, f32::max)
            .min(480.0);
        let numbers_col = number_w.iter().copied().fold(0.0, f32::max);
        let text = ui.visuals().text_color();
        let menu = ui.menu_button(RichText::new("☰ Contents").size(15.0), |ui| {
            egui::ScrollArea::vertical()
                .max_height(ui.ctx().content_rect().height() * 0.7)
                .show(ui, |ui| {
                    for (i, e) in self.epub.toc.iter().enumerate() {
                        let mut job = egui::text::LayoutJob::default();
                        job.append(
                            &self.toc_labels[i],
                            indent(e),
                            egui::TextFormat::simple(font.clone(), text),
                        );
                        // Gap to the numbers' column, numbers right-aligned in it.
                        let gap = (names_col - indent(e) - name_w[i]).max(0.0)
                            + 28.0
                            + (numbers_col - number_w[i]);
                        job.append(
                            &numbers[i],
                            gap,
                            egui::TextFormat {
                                valign: egui::Align::Center,
                                ..egui::TextFormat::simple(small.clone(), theme::BASE1)
                            },
                        );
                        let r = ui
                            .selectable_label(current == Some(i), job)
                            .on_hover_text("Its length: Chinese characters, letters and digits");
                        // Opened at the reader's place.
                        if scroll && current == Some(i) {
                            r.scroll_to_me(Some(egui::Align::Center));
                        }
                        if i == 5 {
                            toc_item = Some(r.rect);
                        }
                        if r.clicked() {
                            go = Some(i);
                            ui.close();
                        }
                    }
                });
        });
        self.probe.contents = Some(menu.response.rect);
        self.probe.toc_item = toc_item;
        self.contents_shown = menu.inner.is_some();
        if let Some(i) = go {
            let e = self.epub.toc[i].clone();
            self.forward = true;
            self.go_chapter(
                e.spine,
                e.fragment.map_or(Target::Offset(0), Target::Anchor),
            );
            self.save();
        }
    }

    fn settings_menu(&mut self, ui: &mut egui::Ui) {
        let before = self.settings.clone();
        // The panel's open state is kept here, not in egui's memory: egui
        // keeps one open popup per window, so opening a font list (a popup)
        // would close a popup panel. It ignores clicks and closes with its
        // button, Esc or a click on the page.
        let button =
            ui.selectable_label(self.settings_open, RichText::new("Aa").size(15.0).strong());
        if button.clicked() {
            self.settings_open = !self.settings_open;
        }
        self.probe.aa = Some(button.rect);
        self.probe.serif_box = None;
        self.probe.serif_item = None;
        let mut open = self.settings_open;
        egui::Popup::menu(&button)
            .id(egui::Id::unique("reader-settings"))
            .open_bool(&mut open)
            .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
            .show(|ui| {
            ui.set_min_width(300.0);
            let fams = fonts::families();
            egui::Grid::new("reader-settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                for serif in [true, false] {
                    ui.label(if serif { "Serif" } else { "Sans-serif" });
                    let current = if serif { self.settings.serif.clone() } else { self.settings.sans.clone() };
                    let shown = current.clone().unwrap_or_else(|| {
                        fams.as_ref()
                            .map(|f| fonts::default_choice(f, serif, self.traditional))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "App font".into())
                    });
                    let mut chosen = current.clone();
                    let mut item = None;
                    let combo = egui::ComboBox::from_id_salt(("reader-font", serif))
                        .selected_text(&shown)
                        .width(220.0)
                        .height(420.0)
                        .show_ui(ui, |ui| {
                            match &fams {
                                None => {
                                    ui.label("Looking for fonts…");
                                }
                                Some(f) => {
                                    for (i, fam) in f.iter().enumerate() {
                                        let sel = shown.eq_ignore_ascii_case(&fam.name);
                                        let r = ui.selectable_label(sel, fam.label());
                                        if r.clicked() {
                                            chosen = Some(fam.name.clone());
                                        }
                                        if i == 2 {
                                            item = Some((r.rect, fam.name.clone()));
                                        }
                                    }
                                }
                            }
                        });
                    if serif {
                        self.probe.serif_box = Some(combo.response.rect);
                        self.probe.serif_item = item;
                    }
                    if chosen != current {
                        if serif {
                            self.settings.serif = chosen;
                        } else {
                            self.settings.sans = chosen;
                        }
                    }
                    ui.end_row();
                }
                ui.label("Body text");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.settings.sans_default, false, "Serif");
                    ui.radio_value(&mut self.settings.sans_default, true, "Sans-serif");
                })
                .response
                .on_hover_text("For text whose book doesn't name a font");
                ui.end_row();
                ui.label("Size");
                ui.add(egui::Slider::new(&mut self.settings.size, 12.0..=40.0).step_by(1.0).suffix(" pt"));
                ui.end_row();
                ui.label("Line spacing");
                ui.add(egui::Slider::new(&mut self.settings.line_spacing, 1.2..=2.6).step_by(0.05).fixed_decimals(2))
                    .on_hover_text("The least distance between lines, in text heights; books that ask for more get more");
                ui.end_row();
                ui.label("Margins");
                ui.add(egui::Slider::new(&mut self.settings.margins, 0.4..=2.0).step_by(0.1).fixed_decimals(1));
                ui.end_row();
                ui.label("Pages");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.settings.spread, false, "One");
                    ui.radio_value(&mut self.settings.spread, true, "Two side by side");
                })
                .response
                .on_hover_text("Two pages when the window is wide enough");
                ui.end_row();
            });
        });
        self.settings_open = open;
        if self.settings != before {
            self.dirty = true;
        }
    }
}

/// `12345` as `12,345`.
fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The table of contents' labels, numbered in front by position at their level when
/// the book gives several entries the same name (some JJWXC books title every
/// chapter alike: 偷偷, 偷偷, …), so they can be told apart.
fn toc_labels(toc: &[epub::TocEntry]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let repeats = toc.iter().any(|e| !seen.insert(e.label.as_str()));
    if !repeats {
        return toc.iter().map(|e| e.label.clone()).collect();
    }
    let mut counters: Vec<usize> = Vec::new();
    toc.iter()
        .map(|e| {
            counters.resize(e.depth + 1, 0);
            counters[e.depth] += 1;
            format!("{}. {}", counters[e.depth], e.label)
        })
        .collect()
}

/// Where the pages go in `area`.
fn geometry(settings: &Settings, area: Rect) -> Geometry {
    let base = settings.size;
    let pad_x = (base * 2.2 * settings.margins).max(12.0);
    let pad_top = (base * 2.4 * settings.margins).max(24.0);
    let pad_bottom = (base * 2.4 * settings.margins).max(28.0);
    let desk = 18.0;
    let max_text = base * 34.0;
    let min_spread_text = base * 20.0;
    let h = (area.height() - 2.0 * desk).max(200.0);
    let spread = settings.spread
        && area.width() - 3.0 * desk >= 2.0 * (min_spread_text + 2.0 * pad_x)
        && area.width() > area.height() * 1.15;
    let page_w = if spread {
        ((area.width() - 3.0 * desk) / 2.0).min(max_text + 2.0 * pad_x)
    } else {
        (area.width() - 2.0 * desk).min(max_text + 2.0 * pad_x)
    }
    .max(160.0);
    // Text width a whole number of characters: CJK lines then fill it
    // without stretching, and columns of characters line up.
    let text_w = ((page_w - 2.0 * pad_x) / base).floor().max(4.0) * base;
    let inset_x = (page_w - text_w) / 2.0;
    let content = vec2(text_w, (h - pad_top - pad_bottom).max(base * 3.0));
    let total_w = if spread { 2.0 * page_w } else { page_w };
    let x0 = area.center().x - total_w / 2.0;
    let y0 = area.top() + desk;
    let p0 = Rect::from_min_size(pos2(x0, y0), vec2(page_w, h));
    let p1 = p0.translate(vec2(page_w, 0.0));
    Geometry {
        pages: [p0, p1],
        spread,
        inset: vec2(inset_x, pad_top),
        content,
    }
}

fn paint_item(
    images: &mut images::Images,
    painter: &egui::Painter,
    item: &Item,
    off: Vec2,
    ppp: f32,
) {
    match item {
        Item::Text { pos, galley, faux } => {
            painter.galley(*pos + off, galley.clone(), theme::INK);
            if let Some((g, dx)) = faux {
                painter.galley(*pos + off + vec2(*dx, 0.0), g.clone(), theme::INK);
            }
        }
        Item::Image { rect, path } => {
            let r = rect.translate(off);
            let px = [
                (r.width() * ppp).ceil() as u32,
                (r.height() * ppp).ceil() as u32,
            ];
            match images.texture(path, px) {
                Some(id) => {
                    painter.image(
                        id,
                        r,
                        Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                None => {
                    painter.rect_filled(r, 2.0, theme::BASE2);
                    painter
                        .ctx()
                        .request_repaint_after(std::time::Duration::from_millis(50));
                }
            }
        }
        Item::Fill { rect, color } => {
            if rect.is_positive() {
                painter.rect_filled(rect.translate(off), 0.0, *color);
            }
        }
        Item::Line {
            a,
            b,
            width,
            color,
            style,
        } => {
            let (a, b) = (*a + off, *b + off);
            let stroke = egui::Stroke::new(*width, *color);
            match style {
                LineStyle::Dashed => {
                    painter.extend(egui::Shape::dashed_line(
                        &[a, b],
                        stroke,
                        width * 4.0,
                        width * 3.0,
                    ));
                }
                LineStyle::Dotted => {
                    painter.extend(egui::Shape::dotted_line(
                        &[a, b],
                        *color,
                        width * 2.5,
                        width / 2.0,
                    ));
                }
                LineStyle::Double => {
                    let thin = egui::Stroke::new((width / 3.0).max(0.6), *color);
                    let n = (b - a).normalized().rot90() * (width / 3.0);
                    painter.line_segment([a - n, b - n], thin);
                    painter.line_segment([a + n, b + n], thin);
                }
                _ => {
                    painter.line_segment([a, b], stroke);
                }
            }
        }
    }
}

fn painter_free(ui: &egui::Ui) -> egui::Painter {
    ui.painter().clone()
}

fn centered_note(ui: &egui::Ui, area: Rect, text: &str) {
    ui.painter().text(
        area.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(18.0),
        theme::BASE00,
    );
}

/// A sheet of paper on the desk; in a spread, a soft fold at the spine.
fn draw_page(ui: &egui::Ui, r: Rect, side: usize, spread: bool) {
    let p = ui.painter();
    let shadow = egui::epaint::Shadow {
        offset: [0, 2],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(34),
    };
    p.add(shadow.as_shape(r, 3));
    p.rect_filled(r, 3.0, theme::BASE3);
    if spread {
        // Shade toward the fold.
        let fold_w = 26.0;
        let (x_fold, dir) = if side == 0 {
            (r.right(), -1.0)
        } else {
            (r.left(), 1.0)
        };
        let steps = 10;
        for k in 0..steps {
            let t0 = k as f32 / steps as f32;
            let a = ((1.0 - t0) * (1.0 - t0) * 16.0) as u8;
            let x0 = x_fold + dir * fold_w * t0;
            let x1 = x_fold + dir * fold_w * (t0 + 1.0 / steps as f32);
            let rr = Rect::from_x_y_ranges(x0.min(x1)..=x0.max(x1), r.y_range());
            p.rect_filled(
                rr,
                0.0,
                Color32::from_rgba_unmultiplied(0x58, 0x4e, 0x30, a),
            );
        }
    }
}

/// Solarized Light for the reader's bar, menus and popups.
fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::light();
    let fill = Color32::from_rgb(0xe6, 0xdf, 0xc8);
    let hover = Color32::from_rgb(0xdc, 0xd4, 0xbb);
    let active = Color32::from_rgb(0xd2, 0xc9, 0xae);
    v.panel_fill = theme::BASE2;
    v.window_fill = theme::BASE3;
    v.extreme_bg_color = theme::BASE3;
    v.faint_bg_color = Color32::from_rgb(0xf5, 0xef, 0xdc);
    v.code_bg_color = theme::BASE2;
    v.hyperlink_color = theme::BLUE;
    v.override_text_color = Some(theme::BASE01);
    v.window_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0xd6, 0xce, 0xb5));
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(0x26, 0x8b, 0xd2, 90);
    v.selection.stroke = egui::Stroke::new(1.0, theme::BLUE);
    v.widgets.noninteractive.bg_fill = theme::BASE3;
    v.widgets.noninteractive.weak_bg_fill = theme::BASE3;
    v.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0, Color32::from_rgb(0xdd, 0xd5, 0xbd));
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, theme::BASE01);
    for (w, c) in [
        (&mut v.widgets.inactive, fill),
        (&mut v.widgets.hovered, hover),
        (&mut v.widgets.active, active),
        (&mut v.widgets.open, hover),
    ] {
        w.bg_fill = c;
        w.weak_bg_fill = c;
        w.bg_stroke = egui::Stroke::NONE;
        w.fg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0x07, 0x36, 0x42));
    }
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, theme::BASE1);
    v
}
