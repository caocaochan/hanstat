//! The EPUB reader: a paginated view of a Chinese book with its own layout,
//! styles and images, on a page in the colour theme chosen (`theme.rs`).
//!
//! Chapters are parsed and styled on a background thread (the open one
//! first); the open chapter is laid out on the UI thread when it is shown,
//! with the fonts chosen in the reader, and drawn page by page.

mod anki;
mod css;
mod dicts_ui;
pub(crate) mod epub;
pub(crate) mod fonts;
mod goal_sound;
mod images;
mod layout;
mod markup;
mod popup;
#[cfg(test)]
mod preview;
mod render;
mod stats_ui;
mod style;
mod theme;
mod tracker;
mod tree;
mod tts;
pub(crate) use stats_ui::mins as stats_mins;
pub(crate) use tts::audiobook::{Action as AudiobookAction, Audiobook};

use eframe::egui::{self, Color32, Key, Pos2, Rect, RichText, Vec2, pos2, vec2};
use epub::Epub;
use layout::{ChapterLayout, Item, Params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use style::LineStyle;
use tree::Chapter;

use theme::Theme;

/// Characters of text in the EPUB file `path` as the reader counts them:
/// what its places' offsets are counted in (the reading path turns a place
/// into a share of the file with it).
pub(crate) fn chapter_text_len(epub: &Epub, path: &str) -> usize {
    tree::build(epub, path, &mut tree::SheetCache::default()).text_len
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
    /// The colour theme's id.
    theme: String,
    /// The dictionary popup's theme: [`popup::SAME_AS_PAGES`] or a theme's
    /// id. Settings saved before there was a choice keep the popup as it
    /// was, Catppuccin Mocha.
    #[serde(default = "popup_theme_before")]
    popup_theme: String,
    /// Where each book was left, by its Library id: kept from before the
    /// reading-stats database, and used again only if it can't be read.
    positions: HashMap<String, Position>,
    /// The key held to look up the word under the pointer.
    lookup_key: popup::LookupKey,
    /// The frequency list whose ranks order the popup's entries (its id).
    freq_sort: Option<String>,
    /// Show frequency ranks in the popup.
    show_freq: bool,
    /// Look for dictionary updates once a day.
    check_updates: bool,
    /// Install dictionary updates found, without asking.
    auto_install: bool,
    /// The popup's size as dragged by its corner (width, height of its
    /// entries' area); `None`: the default.
    popup_size: Option<[f32; 2]>,
    /// Anki cards from the popup.
    anki: anki::CardSettings,
    /// Reading aloud.
    tts: tts::TtsSettings,
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
            theme: theme::THEMES[0].id.to_owned(),
            popup_theme: popup::SAME_AS_PAGES.to_owned(),
            positions: HashMap::new(),
            lookup_key: popup::LookupKey::Shift,
            freq_sort: None,
            show_freq: true,
            check_updates: true,
            auto_install: false,
            popup_size: None,
            anki: anki::CardSettings::default(),
            tts: tts::TtsSettings::default(),
        }
    }
}

fn popup_theme_before() -> String {
    "catppuccin-mocha".to_owned()
}

impl Settings {
    fn path() -> std::path::PathBuf {
        hanstat::models::app_dir().join("reader.json")
    }
    /// Missing → defaults; unreadable → defaults, the file kept aside and
    /// reported ([`hanstat::saver::load_json`]).
    fn load() -> Settings {
        hanstat::saver::load_json(&Self::path()).unwrap_or_default()
    }
    fn save(&self) -> Result<(), String> {
        hanstat::saver::write(&Self::path(), serde_json::to_vec_pretty(self).unwrap())
    }
}

/// Widgets of the settings panel as drawn in the last frame.
#[derive(Default)]
struct Probe {
    aa: Option<Rect>,
    /// The theme box, while the panel is open.
    theme_box: Option<Rect>,
    /// The theme list is open: its entries, their themes and whether each
    /// is in full view (not scrolled away).
    theme_items: Vec<(Rect, usize, bool)>,
    /// The popup theme box, while the panel is open.
    popup_theme_box: Option<Rect>,
    /// The popup theme list is open: its "Same as pages" entry, and the
    /// themes as in `theme_items`.
    popup_same: Option<Rect>,
    popup_theme_items: Vec<(Rect, usize, bool)>,
    /// The serif font box, while the panel is open.
    serif_box: Option<Rect>,
    /// The serif list is open: one of its entries and its family.
    serif_item: Option<(Rect, String)>,
    contents: Option<Rect>,
    /// The contents menu is open: its sixth entry.
    toc_item: Option<Rect>,
    /// The pages shown: (page, flow → screen offset).
    sides: Vec<(usize, Vec2)>,
    #[cfg(test)]
    text_menu: [Option<Rect>; 2],
}

/// A line the app puts in the reader's bar, with an optional button.
pub struct Notice {
    pub text: String,
    pub button: Option<&'static str>,
    pub hover: String,
}

/// Where to go once a chapter is laid out.
#[derive(Clone, Debug)]
enum Target {
    Offset(usize),
    /// A character of [`Chapter::chars`] (the sentence read aloud).
    Src(u32),
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

#[derive(Clone, Copy)]
struct TextMenu {
    position: Pos2,
    target: Option<popup::Spot>,
}

pub struct Reader {
    epub: Arc<Epub>,
    book_id: String,
    traditional: bool,
    settings: Settings,
    /// Settings to keep on disk when opened with the audiobook's voice.
    session_tts_default: Option<tts::TtsSettings>,
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
    /// The dictionary popup.
    dict: popup::Popup,
    /// The Dictionaries window.
    dicts: dicts_ui::Manager,
    /// Reading aloud.
    tts: tts::Tts,
    /// The primary press and the selected range, in chapter characters.
    selection_anchor: Option<u32>,
    selection: Option<Range<u32>>,
    /// The click position and its sentence, fixed for the life of the menu.
    text_menu: Option<TextMenu>,
    /// Reading stats: the tracker, the Stats panel and what they show.
    tracker: tracker::Tracker,
    stats_open: bool,
    stats: stats_ui::Cache,
    /// The app's reading speed (Chinese characters a minute: measured, or
    /// set by hand in Settings), set by the app each frame.
    pub cpm: f32,
    pub cpm_by_hand: bool,
    /// Paged past the book's last page for the first time.
    finished: bool,
    /// A line the app shows in the bar (a DuChinese story's new chapters),
    /// set by the app each frame, and its button clicked.
    pub notice: Option<Notice>,
    notice_clicked: bool,
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
        Self::open_at(path, book_id, traditional, None, ctx)
    }

    fn open_with_voice(
        path: &Path,
        book_id: &str,
        traditional: bool,
        spine: usize,
        speech: &tts::Speech,
        ctx: &egui::Context,
    ) -> Result<Reader, String> {
        Self::open_at(path, Some(book_id), traditional, Some((spine, speech)), ctx)
    }

    fn open_at(
        path: &Path,
        book_id: Option<&str>,
        traditional: bool,
        start: Option<(usize, &tts::Speech)>,
        ctx: &egui::Context,
    ) -> Result<Reader, String> {
        let epub = Arc::new(Epub::open(path)?);
        let mut settings = Settings::load();
        let book_id = book_id
            .map(str::to_owned)
            .unwrap_or_else(|| path.display().to_string());
        let (pos, session_tts_default) = if let Some((spine, speech)) = start {
            let previous = settings.tts.clone();
            settings.tts.use_speech(speech, traditional);
            (Position { spine, offset: 0 }, Some(previous))
        } else {
            // The database first; reader.json has places saved before it.
            let saved = crate::reading_stats::left_at(&book_id).map(|p| Position {
                spine: p.spine as usize,
                offset: p.offset as usize,
            });
            (
                saved
                    .or_else(|| settings.positions.get(&book_id).copied())
                    .unwrap_or_default(),
                None,
            )
        };
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
        let tts = tts::Tts::new(
            chapters.clone(),
            chapter_labels(&epub, &toc_labels),
            traditional,
            &epub.title,
            &book_id,
            ctx,
        );
        let mut dict = popup::Popup::new(ctx, settings.freq_sort.clone());
        dict.cards.book = Arc::from(epub.title.as_str());
        dict.sync_cards(&settings.anki);
        let mut dicts = dicts_ui::Manager::new(ctx, &dict);
        dicts.install_default();
        dicts.auto_check(ctx, &settings);
        fonts::prefetch();
        // The reader's colours for every widget and popup while it is open.
        let theme = ctx.theme();
        let prev = ctx.style_of(theme);
        apply_theme(ctx, theme, theme::index(&settings.theme));
        let tracker = crate::reading_stats::with(|s| {
            tracker::Tracker::new(&book_id, &epub.title, s, crate::reading_stats::now())
        });
        Ok(Reader {
            tracker,
            stats_open: false,
            stats: stats_ui::Cache::default(),
            cpm: 300.0,
            cpm_by_hand: false,
            finished: false,
            notice: None,
            notice_clicked: false,
            images: images::Images::new(epub.clone()),
            epub,
            book_id,
            traditional,
            settings,
            session_tts_default,
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
            dict,
            dicts,
            tts,
            selection_anchor: None,
            selection: None,
            text_menu: None,
        })
    }

    /// Restore the app's look and remember the place.
    pub fn close(mut self, ctx: &egui::Context) {
        egui::Popup::close_id(ctx, egui::Id::unique("reader-text-menu"));
        self.end_stats();
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
        // The place goes to the reading-stats database; to reader.json only
        // while that can't be read (it is never written to then).
        if crate::reading_stats::problem().is_none() {
            crate::reading_stats::set_left_at(&self.book_id, self.spine, self.offset);
            if let Err(e) = crate::reading_stats::save() {
                self.error = Some(e);
            }
        } else {
            self.settings.positions.insert(
                self.book_id.clone(),
                Position {
                    spine: self.spine,
                    offset: self.offset,
                },
            );
        }
        let result = if let Some(default) = &self.session_tts_default {
            Settings {
                tts: default.clone(),
                ..self.settings.clone()
            }
            .save()
        } else {
            self.settings.save()
        };
        if let Err(e) = result {
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
        let spine = i.min(self.epub.spine.len() - 1);
        if self.spine != spine {
            self.selection = None;
            self.selection_anchor = None;
        }
        self.spine = spine;
        self.want.store(self.spine, Ordering::Relaxed);
        self.target = Some(target);
    }

    /// The book was just finished (paged past its end), the first time.
    pub fn take_finished(&mut self) -> bool {
        std::mem::take(&mut self.finished)
    }

    pub fn book_id(&self) -> &str {
        &self.book_id
    }

    /// The last chapter is shown.
    pub fn on_last_chapter(&self) -> bool {
        self.spine + 1 >= self.epub.spine.len()
    }

    /// The notice's button was clicked (since last asked).
    pub fn take_notice_click(&mut self) -> bool {
        std::mem::take(&mut self.notice_clicked)
    }

    fn next(&mut self, g: &Geometry) {
        self.forward = true;
        self.turned_at = self.now;
        self.tracker.turned();
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
        } else if self.layouts.contains_key(&self.spine) {
            self.stats_finish();
        }
    }

    fn prev(&mut self, g: &Geometry) {
        self.forward = false;
        self.turned_at = self.now;
        self.tracker.turned();
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

    fn copy_selection(&self, ctx: &egui::Context) {
        if let (Some(range), Some(ch)) = (&self.selection, self.chapter(self.spine)) {
            ctx.copy_text(
                ch.chars[range.start as usize..range.end as usize]
                    .iter()
                    .map(|&c| if c == '\0' { '\n' } else { c })
                    .collect(),
            );
        }
    }

    fn show_text_menu(
        &mut self,
        response: &egui::Response,
        id: egui::Id,
        change: Option<egui::SetOpenCommand>,
    ) {
        let Some(menu) = self.text_menu else { return };
        let play = egui::Popup::menu(response)
            .id(id)
            .open_memory(change)
            .at_position(menu.position)
            .show(|ui| {
                let copy = ui.add_enabled(self.selection.is_some(), egui::Button::new("Copy"));
                let play =
                    ui.add_enabled(menu.target.is_some(), egui::Button::new("Play from here"));
                #[cfg(test)]
                {
                    self.probe.text_menu = [Some(copy.rect), Some(play.rect)];
                }
                if copy.clicked() {
                    self.copy_selection(ui.ctx());
                    ui.close();
                }
                if play.clicked() {
                    ui.close();
                }
                play.clicked()
            })
            .is_some_and(|menu| menu.inner);
        if play && let Some(spot) = menu.target {
            self.tts.play_from(spot.spine, spot.src, &self.settings.tts);
        }
    }

    /// Draw the reader. `false` once it is closed.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        self.now = now;
        let th = theme::get(&ctx);
        self.images.next_frame();
        self.images.poll(&ctx);
        let mut open = true;
        self.dict.warm(&ctx);
        self.dict.size = self.settings.popup_size.map(|[w, h]| vec2(w, h));
        self.dict.theme.clone_from(&self.settings.popup_theme);
        self.dict.poll();
        self.dict.anchor = None;
        self.dicts.poll(&ctx, &self.dict, &self.settings);
        self.tts.poll(&self.settings.tts);
        // Dictionaries dropped on the reader are imported.
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")))
                .collect()
        });
        if !dropped.is_empty() {
            self.dicts.import(dropped);
            self.dicts.open = true;
        }
        // A click that closes a popup doesn't also turn the page.
        let menus_open = egui::Popup::is_any_open(&ctx)
            || self.settings_open
            || self.stats_open
            || self.tts.panel_open
            || self.tts.local_window_open()
            || self.tts.design_window_open()
            || self.dicts.open
            || self.dict.cards.setup_open;
        self.stats_frame(&ctx, menus_open);
        let dict_open = self.dict.is_open();
        let popup_was_open = menus_open || dict_open;
        let text_menu_id = egui::Id::unique("reader-text-menu");
        let text_menu_open = egui::Popup::is_id_open(&ctx, text_menu_id);
        let text_input_busy = text_menu_open || self.selection_anchor.is_some();
        #[cfg(test)]
        {
            self.probe.text_menu = [None; 2];
        }
        // Esc closes an open panel, menu or the dictionary popup (egui closes
        // its menus itself while drawing them), never the reader with it.
        let esc_for_popup = popup_was_open && ctx.input(|i| i.key_pressed(Key::Escape));
        if esc_for_popup {
            self.settings_open = false;
            self.stats_open = false;
            if self.tts.design_window_open() {
                self.tts.close_design_window();
            } else if self.tts.local_window_open() && !self.tts.panel_open {
                self.tts.close_local_window();
            }
            self.tts.panel_open = false;
            if self.dict.cards.setup_open {
                self.dict.cards.setup_open = false;
            } else if self.dicts.open {
                self.dicts.open = false;
            } else if !menus_open {
                self.dict.close_top();
            }
        }
        let lookup_key = self.settings.lookup_key;
        let key_held = ctx.input(|i| lookup_key.held(&i.modifiers));
        let mut geom_for_keys: Option<Geometry> = self.last_geom;
        let mut play_page = false;

        egui::Panel::top("reader-bar")
            .frame(
                egui::Frame::new()
                    .fill(th.bar)
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
                            .color(th.text)
                            .strong(),
                    );
                    if let Some(n) = &self.notice {
                        ui.add_space(12.0);
                        ui.label(RichText::new(&n.text).size(14.0).color(th.accent))
                            .on_hover_text(&n.hover);
                        if let Some(b) = n.button
                            && ui
                                .button(RichText::new(b).size(14.0))
                                .on_hover_text(&n.hover)
                                .clicked()
                        {
                            self.notice_clicked = true;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.settings_menu(ui);
                        self.stats_menu(ui);
                        self.contents_menu(ui);
                        let n = self.dicts.updates();
                        let label = if self.dicts.working() {
                            "Dictionaries …".to_owned()
                        } else if n > 0 {
                            format!("Dictionaries ({n} update{})", if n == 1 { "" } else { "s" })
                        } else {
                            "Dictionaries".to_owned()
                        };
                        let b = ui
                            .selectable_label(self.dicts.open, RichText::new(label).size(14.0))
                            .on_hover_text(format!(
                                "Look up a word: hold {} with the pointer on it",
                                self.settings.lookup_key.label()
                            ));
                        if b.clicked() {
                            self.dicts.open = !self.dicts.open;
                        }
                        let before = self.settings.tts.clone();
                        let read = self.tts.bar(ui);
                        if let Some(tts::Action::PlayPage) =
                            self.tts.panel(&read, &mut self.settings.tts, self.spine)
                        {
                            play_page = true;
                        }
                        if self.settings.tts != before {
                            self.dirty = true;
                        }
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
                            ui.label(RichText::new(format!("{:.0}%", p * 100.0)).color(th.muted))
                                .on_hover_text("Of the book's text, before this page");
                        }
                    });
                });
            });

        egui::CentralPanel::no_frame().show(ui, |ui| {
            let area = ui.max_rect();
            ui.painter().rect_filled(area, 0.0, th.desk);
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
                text: th.ink,
                link: th.accent,
                rule: th.muted,
                page: th.page,
                dark: th.dark,
            };
            // Not while a slider or the window edge is being dragged.
            let settled = (now - self.resized_at > 0.15
                && !text_input_busy
                && !ctx.input(|i| i.pointer.any_down()))
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
                    Target::Src(s) => l.page_of_src(s).unwrap_or(l.pages.len() - 1),
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
            self.stats_place(&l);
            self.tts.page_src = l.page_first_src(self.page).unwrap_or(0);
            self.tts.page_end = l.page_first_src(self.page + step).unwrap_or(u32::MAX);
            if play_page {
                let src = l.page_first_src(self.page).unwrap_or(0);
                self.tts.play_from(self.spine, src, &self.settings.tts);
            }
            let resp = ui.interact(
                area,
                egui::Id::unique("reader-page"),
                egui::Sense::click_and_drag(),
            );
            let primary_click = resp.clicked_by(egui::PointerButton::Primary);
            let secondary_click = resp.clicked_by(egui::PointerButton::Secondary);
            let spoken_chapter = if !text_input_busy
                && !resp.is_pointer_button_down_on()
                && !primary_click
                && !secondary_click
            {
                self.follow_reading(&l, step)
            } else {
                None
            };
            let reading = self
                .tts
                .at()
                .filter(|(sp, _)| *sp == self.spine)
                .map(|(_, r)| r);
            let label = self.toc_label().map(str::to_owned);
            // Where each page's text is drawn: (page, flow → screen offset, clip).
            let sides: Vec<(usize, Vec2, Rect)> = (0..step)
                .filter_map(|side| {
                    let idx = self.page + side;
                    if idx >= l.pages.len() {
                        return None;
                    }
                    let origin = g.pages[side].min + g.inset;
                    let mut off = origin.to_vec2();
                    if l.pages[idx].center {
                        off.y += ((g.content.y - l.page_height(idx)) / 2.0).max(0.0);
                    }
                    let off = off + l.page_items(idx).1;
                    Some((idx, off, Rect::from_min_size(origin, g.content).expand(4.0)))
                })
                .collect();
            if self
                .selection
                .as_ref()
                .is_some_and(|range| !sides.iter().any(|(idx, _, _)| l.shows_text(*idx, range)))
            {
                self.selection = None;
            }
            let hit_text = |p: Pos2| {
                sides.iter().find_map(|&(idx, off, clip)| {
                    clip.contains(p)
                        .then(|| l.char_at(idx, p - off))
                        .flatten()
                        .map(|src| (src, idx, off))
                })
            };
            let hit_link = |p: Pos2| {
                sides.iter().find_map(|&(idx, off, _)| {
                    l.page_links(idx)
                        .find(|link| link.rect.translate(off).contains(p))
                        .map(|link| link.target)
                })
            };
            let primary = egui::PointerButton::Primary;
            if !popup_was_open
                && resp.is_pointer_button_down_on()
                && let Some(p) = ctx.input(|i| {
                    i.pointer
                        .button_pressed(primary)
                        .then(|| i.pointer.press_origin())
                        .flatten()
                })
                && let Some((_, idx, off)) = hit_text(p)
            {
                self.selection_anchor = l.caret_near(idx, p - off);
                resp.request_focus();
            }
            let dragged = resp.dragged_by(primary);
            if let Some(anchor) = self.selection_anchor
                && (dragged || resp.drag_stopped_by(primary))
                && let Some(p) = resp.interact_pointer_pos()
                && let Some((_, end)) = sides
                    .iter()
                    .filter_map(|&(idx, off, clip)| {
                        let distance = (p.x - p.x.clamp(clip.left(), clip.right())).abs();
                        l.caret_near(idx, p - off).map(|end| (distance, end))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))
            {
                self.selection = (anchor != end).then(|| anchor.min(end)..anchor.max(end));
            }
            if !ctx.input(|i| i.pointer.button_down(primary)) {
                self.selection_anchor = None;
            }
            if primary_click && !popup_was_open {
                self.selection = None;
            }
            let menu_position = if secondary_click && !menus_open && !text_input_busy {
                ctx.input(|i| {
                    i.events.iter().rev().find_map(|event| match event {
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Secondary,
                            pressed: false,
                            ..
                        } => Some(*pos),
                        _ => None,
                    })
                })
                .filter(|&p| {
                    hit_text(p).is_some()
                        || (self.selection.is_some()
                            && g.pages[..sides.len()].iter().any(|page| page.contains(p)))
                })
            } else {
                None
            };
            if let Some(p) = menu_position {
                self.dict.close();
                let target = hit_text(p).and_then(|(src, _, _)| {
                    let ch = self.chapter(self.spine)?;
                    let sentences = tts::sentences(&ch.chars);
                    let sentence = sentences.get(tts::sentence_at(&sentences, src))?;
                    sentence.contains(&src).then_some(popup::Spot {
                        spine: self.spine,
                        src: sentence.start,
                    })
                });
                self.text_menu = Some(TextMenu {
                    position: p,
                    target,
                });
            }
            if menu_position.is_some() || text_menu_open {
                let change = if menu_position.is_some() {
                    Some(egui::SetOpenCommand::Bool(true))
                } else if primary_click {
                    Some(egui::SetOpenCommand::Bool(false))
                } else {
                    None
                };
                self.show_text_menu(&resp, text_menu_id, change);
            } else if !popup_was_open
                && resp.has_focus()
                && !ctx.text_edit_focused()
                && ctx.input(|i| i.events.contains(&egui::Event::Copy))
            {
                self.copy_selection(&ctx);
            }
            if self.text_menu.is_some() && !egui::Popup::is_id_open(&ctx, text_menu_id) {
                self.text_menu = None;
            }
            if !popup_was_open && menu_position.is_none() {
                if dragged && self.selection_anchor.is_some() {
                    ctx.set_cursor_icon(egui::CursorIcon::Text);
                } else if let Some(p) = resp.hover_pos() {
                    if hit_link(p).is_some() {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    } else if hit_text(p).is_some() {
                        ctx.set_cursor_icon(egui::CursorIcon::Text);
                    }
                }
            }
            // The word under the pointer, with the lookup key held: looked up
            // before the pages are drawn, so its highlight shows this frame.
            if key_held
                && !menus_open
                && menu_position.is_none()
                && self.selection_anchor.is_none()
                && let Some(h) = resp.hover_pos()
                && let Some((src, _, _)) = hit_text(h)
                && let Some(ch) = self.chapter(self.spine)
            {
                let spot = popup::Spot {
                    spine: self.spine,
                    src,
                };
                self.dict.point(spot, &ch.chars, lookup_key, now, &ctx);
            }
            self.probe.sides = sides.iter().map(|(i, o, _)| (*i, *o)).collect();
            let highlight = self.dict.highlight(self.spine);
            let stats_line = self.stats_line(crate::reading_stats::now());
            for side in 0..step {
                let page_rect = g.pages[side];
                draw_page(ui, th, page_rect, side, g.spread);
                let idx = self.page + side;
                let Some(&(_, off, clip)) = sides.iter().find(|s| s.0 == idx) else {
                    continue;
                };
                let items = l.page_items(idx).0;
                let painter = ui.painter_at(clip.expand2(vec2(g.inset.x * 0.5, 0.0)));
                let ppp = ctx.pixels_per_point();
                if let Some(r) = &reading {
                    for r in l.char_rects(idx, r.clone()) {
                        painter.rect_filled(
                            r.translate(off).expand2(vec2(1.0, 0.0)),
                            3.0,
                            th.reading(),
                        );
                    }
                }
                if let Some((src, n)) = highlight {
                    for r in l.char_rects(idx, src..src + n as u32) {
                        let r = r.translate(off);
                        painter.rect_filled(r.expand2(vec2(1.0, 0.0)), 3.0, th.highlight());
                        self.dict.anchor = Some(self.dict.anchor.map_or(r, |a| a.union(r)));
                    }
                }
                for i in items {
                    paint_item(&mut self.images, th, &painter, &l.items[i], off, ppp);
                }
                if let Some(range) = &self.selection {
                    for r in l.char_rects(idx, range.clone()) {
                        painter.rect_filled(r.translate(off), 2.0, ui.visuals().selection.bg_fill);
                    }
                }
                // Running head and folio.
                let small = egui::FontId::proportional((self.settings.size * 0.62).max(11.0));
                if let Some(t) = &label {
                    painter_free(ui).text(
                        pos2(page_rect.center().x, page_rect.top() + g.inset.y * 0.45),
                        egui::Align2::CENTER_CENTER,
                        t,
                        small.clone(),
                        th.muted,
                    );
                }
                let folio = painter_free(ui).text(
                    pos2(page_rect.center().x, page_rect.bottom() - g.inset.y * 0.5),
                    egui::Align2::CENTER_CENTER,
                    format!("{} / {}", idx + 1, l.pages.len()),
                    small,
                    th.muted,
                );
                ui.interact(
                    folio.expand(3.0),
                    egui::Id::unique(("reader-folio", side)),
                    egui::Sense::hover(),
                )
                .on_hover_text(format!(
                    "Page {} of {} in this chapter (pages change with the window, text size and margins)",
                    idx + 1,
                    l.pages.len()
                ));
            }
            // Reading stats under the pages: the session (a click pauses or
            // starts timing) and the time left.
            if let Some(line) = &stats_line {
                let small = egui::FontId::proportional((self.settings.size * 0.62).max(11.0));
                let parts = Self::draw_stats_line(ui, th, &g, step, line, &small);
                for (i, (r, hover, click)) in parts.into_iter().enumerate() {
                    let sense = if click {
                        egui::Sense::click()
                    } else {
                        egui::Sense::hover()
                    };
                    let id = egui::Id::unique(("reader-stats-line", i));
                    let r = ui.interact(r.expand(3.0), id, sense).on_hover_text(hover);
                    if click && !popup_was_open {
                        if r.hovered() {
                            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if r.clicked() {
                            self.stats_toggle();
                        }
                    }
                }
            }
            if primary_click && let Some(p) = resp.interact_pointer_pos() {
                if popup_was_open {
                    self.settings_open = false;
                    self.stats_open = false;
                    self.tts.panel_open = false;
                    self.dict.close();
                } else if let Some(t) = hit_link(p) {
                    if let Some(ch) = self.chapter(self.spine)
                        && let Some(tree::LinkTarget::Chapter { path, fragment }) =
                            ch.links.get(t as usize).cloned()
                    {
                        self.follow(&path, fragment);
                    }
                } else if let Some((src, _, _)) = hit_text(p) {
                    if self.tts.active() {
                        self.tts.play_from(self.spine, src, &self.settings.tts);
                    }
                } else {
                    // Only a click beside the pages turns them: left of them
                    // back, right of them forward. On a page, above, below or
                    // in the gutter it does nothing.
                    let left = g.pages[0].left();
                    let right = if g.spread {
                        g.pages[1].right()
                    } else {
                        g.pages[0].right()
                    };
                    if p.x < left {
                        self.prev(&g);
                    } else if p.x > right {
                        self.next(&g);
                    }
                }
            }
            if let Some((spine, src)) = spoken_chapter {
                self.forward = spine > self.spine;
                self.tracker.turned();
                self.go_chapter(spine, Target::Src(src));
                self.save();
                ctx.request_repaint();
                return;
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
        // With the dictionary popup open they close it too; the wheel over
        // the popup scrolls it instead.
        let over_dict = ctx
            .pointer_hover_pos()
            .is_some_and(|p| self.dict.contains(p));
        if let Some(g) = geom_for_keys
            && !ctx.text_edit_focused()
            && !egui::Popup::is_any_open(&ctx)
            && !self.settings_open
            && !menus_open
            && !text_input_busy
            && self.selection_anchor.is_none()
            && !esc_for_popup
        {
            // With the popup open, ↑ and ↓ scroll it.
            if self.dict.is_open() {
                let (down, up) = ctx.input_mut(|i| {
                    (
                        i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                        i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                    )
                });
                self.dict.scroll_lines += down as i32 - up as i32;
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::P)) {
                self.stats_toggle();
            }
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
            let wheel = if over_dict { 0.0 } else { wheel };
            if fwd || back || home || end || wheel != 0.0 {
                self.dict.close();
            }
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
        // The popup, beside its word; gone with the word (a page turned).
        if self.dict.is_open() {
            if let (Some(_), Some(params)) = (self.dict.anchor, self.params.clone()) {
                self.dict.ui(
                    &ctx,
                    &params,
                    self.settings.show_freq,
                    key_held,
                    lookup_key,
                    now,
                );
                if self.dict.size_changed {
                    self.settings.popup_size = self.dict.size.map(|v| [v.x, v.y]);
                    // Saved once the drag ends (the save waits for the button's release).
                    self.dirty = true;
                }
            } else {
                self.dict.close();
            }
        }
        self.tts.popup(self.dict.is_open(), &self.settings.tts);
        if self.dicts.ui(&ctx, &mut self.dict, &mut self.settings) {
            self.dirty = true;
        }
        if self.dict.cards.window(&ctx, &mut self.settings.anki) {
            self.dict.sync_cards(&self.settings.anki);
            self.dirty = true;
        }
        if self.dirty && !ctx.input(|i| i.pointer.any_down()) {
            self.save();
        }
        if let Some(e) = self.error.take() {
            egui::Window::new("Reader")
                .collapsible(false)
                .show(&ctx, |ui| ui.label(e));
        }
        self.goal_met_effects(&ctx);
        open
    }

    /// Turn to the sentence read aloud when it leaves the pages shown, if
    /// the one before was on them (pages turned away by hand stay).
    /// If it moves to another chapter, return its target to open after painting.
    fn follow_reading(&mut self, l: &ChapterLayout, step: usize) -> Option<(usize, u32)> {
        let (spine, r) = self.tts.at()?;
        let first = (spine == self.spine)
            .then(|| l.page_of_src(r.start))
            .flatten();
        let visible = first.is_some_and(|a| {
            let z = l.page_of_src(r.end.saturating_sub(1)).unwrap_or(a);
            a < self.page + step && z >= self.page
        });
        let key = (spine, r.start);
        if self.tts.last_at == Some(key) {
            self.tts.last_visible = visible;
            return None;
        }
        self.tts.last_at = Some(key);
        if !self.tts.last_visible || visible {
            self.tts.last_visible = visible;
            return None;
        }
        // Followed: still "visible" while the turn happens.
        self.tts.last_visible = true;
        self.turned_at = self.now;
        if spine != self.spine {
            return Some((spine, r.start));
        }
        if let Some(p) = first {
            self.forward = p > self.page;
            self.tracker.turned();
            self.page = p - p % step;
            self.offset = l.page_offset(self.page);
        }
        None
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
        let muted = theme::get(ui.ctx()).muted;
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
                                ..egui::TextFormat::simple(small.clone(), muted)
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
        let button = ui
            .selectable_label(self.settings_open, RichText::new("Aa").size(15.0).strong())
            .on_hover_text("Theme, fonts, text size, line spacing, margins and pages");
        if button.clicked() {
            self.settings_open = !self.settings_open;
        }
        self.probe.aa = Some(button.rect);
        self.probe.theme_box = None;
        self.probe.theme_items.clear();
        self.probe.popup_theme_box = None;
        self.probe.popup_same = None;
        self.probe.popup_theme_items.clear();
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
                ui.label("Theme");
                let current = theme::index(&self.settings.theme);
                let mut chosen = None;
                let mut items = Vec::new();
                let button = ui.add(
                    egui::Button::new(theme::THEMES[current].name)
                        .right_text("⏷")
                        .min_size(vec2(220.0, 0.0)),
                );
                egui::Popup::menu(&button)
                    .id(egui::Id::unique("reader-theme-list"))
                    .show(|ui| chosen = theme_columns(ui, Some(current), &mut items));
                self.probe.theme_box = Some(button.rect);
                self.probe.theme_items = items;
                if let Some(i) = chosen {
                    self.settings.theme = theme::THEMES[i].id.to_owned();
                    let egui_theme = self.prev_style.as_ref().map_or(ui.ctx().theme(), |p| p.0);
                    apply_theme(ui.ctx(), egui_theme, i);
                }
                ui.end_row();
                // The dictionary popup's: the pages' theme, or one of its own.
                ui.label("Popup theme");
                let pages = theme::index(&self.settings.theme);
                let current = theme::THEMES.iter().position(|t| t.id == self.settings.popup_theme);
                let mut chosen = None;
                let mut same = None;
                let mut items = Vec::new();
                let button = ui
                    .add(
                        egui::Button::new(current.map_or("Same as pages", |i| theme::THEMES[i].name))
                            .right_text("⏷")
                            .min_size(vec2(220.0, 0.0)),
                    )
                    .on_hover_text("The dictionary popup's colours");
                egui::Popup::menu(&button)
                    .id(egui::Id::unique("reader-popup-theme-list"))
                    .show(|ui| {
                        // As wide as the two columns below it, in the pages' colours.
                        let w = 2.0 * THEME_COLUMN + ui.spacing().item_spacing.x;
                        let r = ui
                            .allocate_ui(vec2(w, 30.0), |ui| {
                                theme::swatch_as(ui, &theme::THEMES[pages], "Same as pages", current.is_none())
                            })
                            .inner;
                        if r.clicked() {
                            chosen = Some(popup::SAME_AS_PAGES);
                            ui.close();
                        }
                        same = Some(r.rect);
                        if let Some(i) = theme_columns(ui, current, &mut items) {
                            chosen = Some(theme::THEMES[i].id);
                        }
                    });
                self.probe.popup_theme_box = Some(button.rect);
                self.probe.popup_same = same;
                self.probe.popup_theme_items = items;
                if let Some(id) = chosen {
                    self.settings.popup_theme = id.to_owned();
                }
                ui.end_row();
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
/// The first table-of-contents title for each reader chapter.
fn chapter_labels(epub: &Epub, toc_labels: &[String]) -> Vec<String> {
    let mut labels = vec![String::new(); epub.spine.len()];
    for (entry, label) in epub.toc.iter().zip(toc_labels) {
        if let Some(current) = labels.get_mut(entry.spine)
            && current.is_empty()
        {
            *current = label.trim().to_owned();
        }
    }
    labels
}

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
    th: &Theme,
    painter: &egui::Painter,
    item: &Item,
    off: Vec2,
    ppp: f32,
) {
    match item {
        Item::Text { pos, galley, faux } => {
            painter.galley(*pos + off, galley.clone(), th.ink);
            if let Some((g, dx)) = faux {
                painter.galley(*pos + off + vec2(*dx, 0.0), g.clone(), th.ink);
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
                    painter.rect_filled(r, 2.0, th.bar);
                    painter
                        .ctx()
                        .request_repaint_after(std::time::Duration::from_millis(50));
                }
            }
        }
        Item::Fill {
            rect,
            color,
            radius,
        } => {
            if rect.is_positive() {
                painter.rect_filled(rect.translate(off), *radius, *color);
            }
        }
        Item::Frame {
            rect,
            radius,
            width,
            color,
        } => {
            painter.rect_stroke(
                rect.translate(off),
                *radius,
                egui::Stroke::new(*width, *color),
                egui::StrokeKind::Middle,
            );
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
        theme::get(ui.ctx()).subtle,
    );
}

/// A sheet of paper on the desk; in a spread, a soft fold at the spine.
fn draw_page(ui: &egui::Ui, th: &Theme, r: Rect, side: usize, spread: bool) {
    let p = ui.painter();
    let shadow = egui::epaint::Shadow {
        offset: [0, 2],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(if th.dark { 90 } else { 34 }),
    };
    p.add(shadow.as_shape(r, 3));
    p.rect_filled(r, 3.0, th.page);
    let (fold, strength) = th.fold();
    let [fr, fg, fb, _] = fold.to_array();
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
            let a = ((1.0 - t0) * (1.0 - t0) * strength) as u8;
            let x0 = x_fold + dir * fold_w * t0;
            let x1 = x_fold + dir * fold_w * (t0 + 1.0 / steps as f32);
            let rr = Rect::from_x_y_ranges(x0.min(x1)..=x0.max(x1), r.y_range());
            p.rect_filled(rr, 0.0, Color32::from_rgba_unmultiplied(fr, fg, fb, a));
        }
    }
}

/// A theme list's column width.
const THEME_COLUMN: f32 = 210.0;

/// The themes to choose from, each shown in its own colours, light and dark
/// side by side: one column of 16 would scroll (egui sizes a new popup at
/// most 400 points tall). Returns the theme clicked (and closes the list);
/// `items` gets each entry's rect, its theme and whether it is in full view
/// (not scrolled away).
fn theme_columns(
    ui: &mut egui::Ui,
    current: Option<usize>,
    items: &mut Vec<(Rect, usize, bool)>,
) -> Option<usize> {
    let muted = theme::get(ui.ctx()).muted;
    let mut chosen = None;
    ui.horizontal_top(|ui| {
        for dark in [false, true] {
            ui.vertical(|ui| {
                ui.set_width(THEME_COLUMN);
                ui.label(
                    RichText::new(if dark { "Dark" } else { "Light" })
                        .size(12.0)
                        .color(muted),
                );
                for (i, t) in theme::THEMES
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.dark == dark)
                {
                    let r = theme::swatch(ui, t, Some(i) == current);
                    if r.clicked() {
                        chosen = Some(i);
                        ui.close();
                    }
                    items.push((r.rect, i, ui.clip_rect().contains_rect(r.rect)));
                }
            });
        }
    });
    chosen
}

/// Use theme `i` in `ctx`: its colours for the reader's bar, menus and
/// popups (in egui's `theme` style, the one in use).
fn apply_theme(ctx: &egui::Context, egui_theme: egui::Theme, i: usize) {
    theme::set(ctx, i);
    let mut style = (*ctx.style_of(egui_theme)).clone();
    style.visuals = theme::get(ctx).visuals();
    ctx.set_style_of(egui_theme, style);
}
