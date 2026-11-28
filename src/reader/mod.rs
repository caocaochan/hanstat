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
pub(crate) use crate::icons;
mod images;
mod layout;
mod markup;
mod popup;
#[cfg(test)]
mod preview;
mod quote;
mod render;
mod search;
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
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) use popup::DictionarySession;

pub(crate) fn prefetch(ctx: &egui::Context, dictionary: &mut DictionarySession) {
    fonts::prefetch();
    let root = hanstat::dict::root();
    if hanstat::dict::format::Manifest::path(&root).is_file() {
        dictionary.prefetch(ctx, root, Settings::load().freq_sort);
    }
}
use style::LineStyle;
use tree::Chapter;

use theme::Theme;

/// Characters of text in the EPUB file `path` as the reader counts them:
/// what its places' offsets are counted in (the reading path turns a place
/// into a share of the file with it).
/// Whether the reader opens books of this Library format: EPUBs, and plain
/// text as chapter pages ([`Epub::from_text`]).
pub(crate) fn opens(format: &str) -> bool {
    matches!(format, "EPUB" | "TXT")
}

pub(crate) fn chapter_text_len(epub: &Epub, path: &str) -> usize {
    tree::build(epub, path, &mut tree::SheetCache::default()).text_len
}

/// Text lengths of every chapter in `epub`, sharing one [`tree::SheetCache`].
pub(crate) fn chapter_text_lens(epub: &Epub) -> Vec<usize> {
    let mut sheets = tree::SheetCache::default();
    epub.spine
        .iter()
        .map(|href| tree::build(epub, href, &mut sheets).text_len)
        .collect()
}

fn group_digits(n: usize) -> String {
    crate::fmt_int(n as u64)
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
    /// Custom page width in points; `None`: automatic for the window and text size.
    page_width: Option<f32>,
    /// Custom page height in points; `None`: automatic for the window and text size.
    page_height: Option<f32>,
    /// Two pages side by side when the window is wide enough.
    spread: bool,
    /// Traditional vertical-rl reading mode on a landscape page.
    vertical: bool,
    /// The menu bar shows only with the pointer at the top of the window,
    /// over the pages, which are laid out as if it weren't there.
    autohide_bar: bool,
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
    /// The search sidebar's width.
    search_width: f32,
    /// Last layout chosen in the quote PNG exporter.
    quote_layout: quote::QuoteLayout,
    /// Text direction chosen in the quote PNG exporter; `None` follows the reader's mode.
    quote_direction: Option<quote::TextDirection>,
    /// Theme chosen in the quote PNG exporter; `None` follows the reader's theme.
    quote_theme: Option<String>,
    /// Quote text font chosen in the quote PNG exporter; `None` follows the reader's font.
    quote_font: Option<String>,
    /// Book title font chosen in the quote PNG exporter; `None` uses the bundled WenKai,
    /// `Some("")` matches the quote font, otherwise a system font family name.
    quote_title_font: Option<String>,
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
            page_width: None,
            page_height: None,
            spread: true,
            vertical: false,
            autohide_bar: false,
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
            search_width: 340.0,
            quote_layout: quote::QuoteLayout::default(),
            quote_direction: None,
            quote_theme: None,
            quote_font: None,
            quote_title_font: None,
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

/// Drop any legacy saved place for `book_id` in `reader.json` (used when
/// resetting a book's reading data so it doesn't fall back to an old place).
pub(crate) fn forget_position(book_id: &str) {
    if cfg!(test) {
        return;
    }
    let mut settings = Settings::load();
    if settings.positions.remove(book_id).is_some() {
        let _ = settings.save();
    }
}

/// Widgets of the settings panel as drawn in the last frame.
#[cfg(test)]
#[derive(Default)]
struct Probe {
    /// The menu bar, when drawn this frame.
    bar: Option<Rect>,
    /// The Back bar (a search result shown), when drawn this frame.
    back_bar: Option<Rect>,
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
    /// The Vertical radio button, while the panel is open.
    vertical: Option<Rect>,
    contents: Option<Rect>,
    /// The contents menu is open: its sixth entry.
    toc_item: Option<Rect>,
    /// The pages shown: (page, flow → screen offset).
    sides: Vec<(usize, Vec2)>,
    text_menu: [Option<Rect>; 2],
}

/// Cached widths and formatted lengths for the Contents menu.
struct TocCache {
    has_lengths: bool,
    name_w: Vec<f32>,
    numbers: Vec<String>,
    number_w: Vec<f32>,
    ellipsis_w: f32,
    names_col: f32,
    numbers_col: f32,
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
    /// Reflow keeping `offset` as the saved anchor while targeting `src` when
    /// `offset` is shared with a preceding zero-text page (e.g. a cover image).
    ReflowSrc {
        offset: usize,
        src: u32,
    },
    /// A character of [`Chapter::chars`] (the sentence read aloud or search hit).
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
    /// How many chapters the background worker has built so far.
    chapters_built: Arc<AtomicUsize>,
    /// Prefix sums and total text characters once every chapter is built.
    text_totals: Option<(Vec<usize>, Vec<usize>, usize)>,
    /// Prefix sums and total Chinese characters once every chapter is built.
    han_totals: Option<(Vec<u64>, u64)>,
    want: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    layouts: HashMap<usize, Arc<ChapterLayout>>,
    layout_work: Option<(usize, layout::LayoutWork)>,
    params: Option<Params>,
    images: images::Images,
    fonts: Option<fonts::Installed>,
    spine: usize,
    page: usize,
    target: Option<Target>,
    /// Direction of the last move (skipping empty chapters).
    forward: bool,
    offset: usize,
    /// Last known resolved, non-peek reading position, so saving while a
    /// cross-chapter non-offset target is still loading never pairs a new
    /// `spine` with an old chapter's `offset`.
    last_valid_pos: Position,
    /// Flush the reading position to the database once the pending target settles.
    position_needs_save: bool,
    /// When the reading position first changed in memory without a DB flush yet.
    position_dirty_at: Option<f64>,
    prev_style: Option<(egui::Theme, Arc<egui::Style>)>,
    scroll: f32,
    resized_at: f64,
    last_geom: Option<Geometry>,
    /// The geometry the laid-out pages were made for (kept while the
    /// window is resized or a slider dragged).
    drawn: Option<Geometry>,
    /// Settings changed and not yet saved; `persist`: save at all (tests).
    dirty: bool,
    #[cfg(test)]
    persist: bool,
    /// Table of contents entries as shown: numbered when the book repeats names.
    toc_labels: Vec<String>,
    /// Cached widths and formatted lengths for the Contents menu.
    toc_cache: Option<TocCache>,
    /// The contents menu was open last frame.
    contents_shown: bool,
    /// The Aa panel is open.
    settings_open: bool,
    /// Where the settings' widgets were drawn this frame (for tests that click them).
    #[cfg(test)]
    probe: Probe,
    error: Option<String>,
    /// The hidden menu bar ([`Settings::autohide_bar`]): its height when
    /// last drawn, the time it shows until, and the notice it last showed
    /// for.
    bar_h: f32,
    bar_until: f64,
    bar_notice: Option<String>,
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
    /// The "Export quote" PNG dialog, when open.
    quote_export: Option<quote::QuoteDialog>,
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
    /// Ctrl+F: the search sidebar and its results.
    search: search::Search,
    /// A search result is shown: where reading was. Until Back (or "Keep
    /// reading here") the place, reading time and stats stay there.
    peek: Option<Position>,
    /// The match shown, highlighted on its page.
    search_hit: Option<(usize, Range<u32>)>,
    /// Each chapter's name in the table of contents (empty: none).
    chapter_names: Vec<String>,
    /// Where the pages are drawn this frame (the sidebar goes over its left).
    page_area: Option<Rect>,
    /// DuChinese sentence translations revealed by clicking: `(spine, spoiler_id)`.
    revealed_spoilers: HashSet<(usize, u32)>,
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
        dictionary: &mut DictionarySession,
    ) -> Result<Reader, String> {
        Self::open_at(path, book_id, traditional, None, ctx, dictionary)
    }

    fn open_with_voice(
        path: &Path,
        book_id: &str,
        traditional: bool,
        spine: usize,
        speech: &tts::Speech,
        ctx: &egui::Context,
        dictionary: &mut DictionarySession,
    ) -> Result<Reader, String> {
        Self::open_at(
            path,
            Some(book_id),
            traditional,
            Some((spine, speech)),
            ctx,
            dictionary,
        )
    }

    fn open_at(
        path: &Path,
        book_id: Option<&str>,
        traditional: bool,
        start: Option<(usize, &tts::Speech)>,
        ctx: &egui::Context,
        dictionary: &mut DictionarySession,
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
        let chapters_built = Arc::new(AtomicUsize::new(0));
        let want = Arc::new(AtomicUsize::new(spine));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (epub, chapters, chapters_built, want, stop, ctx) = (
                epub.clone(),
                chapters.clone(),
                chapters_built.clone(),
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
                        chapters_built.fetch_add(1, Ordering::Release);
                        if (i as isize - want.load(Ordering::Relaxed) as isize).abs() <= 1 {
                            ctx.request_repaint();
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        let toc_labels = toc_labels(&epub.toc);
        let du_audio = epub
            .duchinese
            .then(|| tts::duchinese::DuBook::open(path, &epub.spine, traditional));
        let tts = tts::Tts::new(
            chapters.clone(),
            chapter_labels(&epub, &toc_labels),
            traditional,
            &epub.title,
            &book_id,
            ctx,
        )
        .with_duchinese(du_audio);
        let mut dict = popup::Popup::new(ctx, settings.freq_sort.clone(), dictionary);
        dict.cards.book = Arc::from(epub.title.as_str());
        let migrated = settings.anki.add_sentence_audio();
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
        let search = search::Search::new(chapters.clone(), ctx, settings.search_width);
        let dirty = migrated || crate::reading_stats::is_hidden_recent(&book_id);
        let chapter_names = chapter_labels(&epub, &toc_labels);
        Ok(Reader {
            search,
            peek: None,
            search_hit: None,
            chapter_names,
            page_area: None,
            revealed_spoilers: HashSet::new(),
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
            chapters_built,
            text_totals: None,
            han_totals: None,
            want,
            stop,
            layouts: HashMap::new(),
            layout_work: None,
            params: None,
            fonts: None,
            spine,
            page: 0,
            target: Some(Target::Offset(pos.offset)),
            forward: true,
            offset: pos.offset,
            last_valid_pos: Position {
                spine,
                offset: pos.offset,
            },
            position_needs_save: false,
            position_dirty_at: None,
            prev_style: Some((theme, prev)),
            scroll: 0.0,
            resized_at: 0.0,
            last_geom: None,
            drawn: None,
            dirty,
            #[cfg(test)]
            persist: true,
            toc_labels,
            toc_cache: None,
            contents_shown: false,
            settings_open: false,
            #[cfg(test)]
            probe: Probe::default(),
            error: None,
            bar_h: 40.0,
            bar_until: 0.0,
            bar_notice: None,
            now: 0.0,
            turned_at: 0.0,
            dict,
            dicts,
            tts,
            selection_anchor: None,
            selection: None,
            text_menu: None,
            quote_export: None,
        })
    }

    #[inline]
    pub(super) fn should_persist(&self) -> bool {
        #[cfg(test)]
        {
            self.persist
        }
        #[cfg(not(test))]
        {
            true
        }
    }

    /// Populate cached prefix sums once every chapter has finished building.
    fn sync_chapter_totals(&mut self) {
        if self.text_totals.is_some() {
            return;
        }
        let n = self.epub.spine.len();
        if self.chapters_built.load(Ordering::Acquire) < n {
            return;
        }
        let c = self.chapters.lock().unwrap();
        let mut text_before = Vec::with_capacity(n);
        let mut text_lens = Vec::with_capacity(n);
        let mut text_total = 0usize;
        let mut han_before = Vec::with_capacity(n);
        let mut han_total = 0u64;
        for ch in c.iter() {
            let Some(ch) = ch.as_ref() else {
                return;
            };
            text_before.push(text_total);
            text_lens.push(ch.text_len);
            text_total += ch.text_len;
            han_before.push(han_total);
            han_total += ch.han_len as u64;
        }
        self.text_totals = Some((text_before, text_lens, text_total));
        self.han_totals = Some((han_before, han_total));
    }

    /// Restore the app's look and remember the place.
    pub fn close(mut self, ctx: &egui::Context) {
        egui::Popup::close_id(ctx, egui::Id::unique("reader-text-menu"));
        // Closed while a search result was shown: reading was elsewhere.
        if let Some(p) = self.peek.take() {
            self.spine = p.spine;
            self.offset = p.offset;
            self.target = None;
            self.last_valid_pos = p;
        }
        self.end_stats();
        self.save();
        if let Some((theme, style)) = self.prev_style.take() {
            ctx.set_style_of(theme, style);
        }
    }

    /// The reading position that should be persisted right now.
    /// Never returns a search-peek location or an unresolved cross-chapter
    /// `(new_spine, old_offset)` pair.
    fn persistent_position(&self) -> Position {
        if let Some(p) = self.peek {
            return p;
        }
        let Some(t) = &self.target else {
            return Position {
                spine: self.spine,
                offset: self.offset,
            };
        };
        match t {
            Target::Offset(o) | Target::ReflowSrc { offset: o, .. } => Position {
                spine: self.spine,
                offset: *o,
            },
            Target::Last | Target::Anchor(_) | Target::Src(_) => {
                if let Some(l) = self.layouts.get(&self.spine)
                    && !l.pages.is_empty()
                {
                    let step = self.drawn.or(self.last_geom).map_or(1, |g| self.step(&g));
                    let mut p = match t {
                        Target::Last => l.pages.len() - 1,
                        Target::Anchor(a) => l.page_of_anchor(a).unwrap_or(0),
                        Target::Src(s) => l.page_of_src(*s).unwrap_or(l.pages.len() - 1),
                        Target::Offset(_) | Target::ReflowSrc { .. } => unreachable!(),
                    };
                    p -= p % step;
                    return Position {
                        spine: self.spine,
                        offset: l.page_offset(p),
                    };
                }
                if let Some(ch) = self.chapter(self.spine) {
                    match t {
                        Target::Last => {
                            let end = ch.chars.iter().filter(|&&c| c != '\0' && c != ' ').count();
                            return Position {
                                spine: self.spine,
                                offset: end,
                            };
                        }
                        Target::Src(s) => {
                            let off = ch.chars[..(*s as usize).min(ch.chars.len())]
                                .iter()
                                .filter(|&&c| c != '\0' && c != ' ')
                                .count();
                            return Position {
                                spine: self.spine,
                                offset: off,
                            };
                        }
                        _ => {}
                    }
                }
                self.last_valid_pos
            }
        }
    }

    fn save_settings(&mut self) {
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

    /// Record the reading position in memory and optionally flush it to disk.
    fn sync_position(&mut self, flush_db: bool) {
        if !flush_db && self.peek.is_some() {
            return;
        }
        let pos = self.persistent_position();
        if self.peek.is_none()
            && !matches!(
                self.target,
                Some(Target::Last | Target::Anchor(_) | Target::Src(_))
            )
        {
            self.last_valid_pos = pos;
        }
        if !self.should_persist() {
            self.position_dirty_at = None;
            return;
        }
        // The place goes to the reading-stats database; to reader.json only
        // while that can't be read (it is never written to then).
        if crate::reading_stats::problem().is_none() {
            crate::reading_stats::set_left_at(&self.book_id, pos.spine, pos.offset);
            if flush_db {
                self.position_dirty_at = None;
                if let Err(e) = crate::reading_stats::save() {
                    self.error = Some(e);
                }
            } else {
                self.position_dirty_at.get_or_insert(self.now);
            }
        } else {
            self.position_dirty_at = None;
            if self.settings.positions.get(&self.book_id) != Some(&pos) {
                self.settings.positions.insert(self.book_id.clone(), pos);
                if flush_db {
                    self.save_settings();
                } else {
                    self.dirty = true;
                }
            }
        }
    }

    pub fn save(&mut self) {
        self.dirty = false;
        if !self.should_persist() {
            return;
        }
        self.sync_position(true);
        self.save_settings();
    }

    fn chapter(&self, i: usize) -> Option<Arc<Chapter>> {
        self.chapters.lock().unwrap().get(i).cloned().flatten()
    }

    /// Build a reflow target that preserves the current reading position
    /// across layout changes (fonts, size, margins, window resize, spread).
    fn reflow_target(&self) -> Target {
        if self.page > 0
            && let Some(l) = self.layouts.get(&self.spine)
            && self.offset == l.page_offset(self.page)
            && l.page_offset(self.page - 1) == self.offset
            && let Some(src) = l.page_first_src(self.page)
        {
            return Target::ReflowSrc {
                offset: self.offset,
                src,
            };
        }
        Target::Offset(self.offset)
    }

    /// Drop cached chapter layouts and ensure the current reading position is
    /// restored once the open chapter lays out again.
    fn invalidate_layouts(&mut self) {
        if self.target.is_none() {
            self.target = Some(self.reflow_target());
        }
        self.layouts.clear();
        self.layout_work = None;
    }

    /// Fonts as chosen; reinstalled when the choice changes. `None` until the
    /// font scan is done and egui has the families.
    fn fonts_ready(&mut self, ctx: &egui::Context) -> Option<fonts::Installed> {
        let Some(fams) = fonts::families() else {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
            return None;
        };
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
            self.fonts = Some(fonts::install(ctx, &fams, &serif, &sans));
            self.invalidate_layouts();
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

    /// Advance the layout of chapter `i` if its tree is ready.
    fn layout(
        &mut self,
        ctx: &egui::Context,
        i: usize,
        deadline: std::time::Instant,
    ) -> Option<Arc<ChapterLayout>> {
        if let Some(l) = self.layouts.get(&i) {
            return Some(l.clone());
        }
        let ch = self.chapter(i)?;
        let params = self.params.clone()?;
        if self
            .layout_work
            .as_ref()
            .is_none_or(|(spine, _)| *spine != i)
        {
            self.layout_work = Some((i, layout::LayoutWork::default()));
        }
        let (_, work) = self.layout_work.as_mut().unwrap();
        let l = ctx.fonts_mut(|f| work.advance(&ch, &params, f, &mut self.images, Some(deadline)));
        let Some(l) = l else {
            ctx.request_repaint();
            return None;
        };
        self.layout_work = None;
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
        match &target {
            Target::Offset(o) | Target::ReflowSrc { offset: o, .. } => {
                self.offset = *o;
                if self.peek.is_none() {
                    self.last_valid_pos = Position { spine, offset: *o };
                }
            }
            Target::Src(_) | Target::Anchor(_) | Target::Last => {}
        }
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

    /// The search sidebar and, while a result is shown, the Back bar.
    fn search_frame(&mut self, ctx: &egui::Context, th: &Theme) {
        let Some(area) = self.page_area else { return };
        // A hidden menu bar comes out when the pointer is in the strip it
        // covers (and a little below): the sidebar and the Back bar start
        // under it, or reaching for them would bring the bar over them.
        let area = if self.settings.autohide_bar {
            area.with_min_y((area.top() + self.bar_h + 12.0).min(area.bottom() - 100.0))
        } else {
            area
        };
        let w = self.search.width;
        if let Some(search::Action::Go(h)) =
            self.search.ui(ctx, area, th, &self.chapter_names, self.now)
        {
            self.search_go(h);
        }
        if self.search.width != w {
            self.settings.search_width = self.search.width;
            self.dirty = true;
        }
        if self.peek.is_some() {
            self.back_bar(ctx, area, th);
        }
    }

    /// Show a search result's page: reading stays where it was (the first
    /// result's peek keeps the place for all that follow).
    fn search_go(&mut self, h: search::Hit) {
        if self.peek.is_none() {
            self.peek = Some(self.persistent_position());
        }
        self.dict.close();
        self.selection = None;
        self.selection_anchor = None;
        self.forward = true;
        self.go_chapter(h.spine, Target::Src(h.range.start));
        self.search_hit = Some((h.spine, h.range));
        // Out of the way of the page; Ctrl+F or 🔍 brings it back.
        self.search.close();
    }

    /// Back to where reading was.
    fn peek_back(&mut self) {
        let Some(p) = self.peek.take() else { return };
        self.tracker.jumped();
        self.search_hit = None;
        self.dict.close();
        self.forward = false;
        self.go_chapter(p.spine, Target::Offset(p.offset));
    }

    /// Read on from the result shown: it becomes the place (a jump: no
    /// characters count for it).
    fn peek_keep(&mut self) {
        if self.peek.take().is_some() {
            // A jump: the next place seen only moves the tracker's base
            // (pages turned while the result was shown don't count).
            self.tracker.jumped();
            self.search_hit = None;
            if matches!(
                self.target,
                Some(Target::Last | Target::Anchor(_) | Target::Src(_))
            ) {
                self.position_needs_save = true;
            } else {
                self.sync_position(true);
            }
        }
    }

    /// "← Back to where you were", over the top of the pages.
    fn back_bar(&mut self, ctx: &egui::Context, area: Rect, th: &Theme) {
        let Some(p) = self.peek else { return };
        let label = self
            .chapter_names
            .get(p.spine)
            .filter(|l| !l.is_empty())
            .cloned();
        let (mut back, mut keep, mut find) = (false, false, false);
        // Over the running head; with the menu bar hidden the top is where
        // the pointer brings it out, so over the footer line instead.
        let footer = self
            .drawn
            .or(self.last_geom)
            .filter(|_| self.settings.autohide_bar)
            .map(|g| g.pages[0].bottom() - g.inset.y * 0.5);
        let (pivot, at) = match footer {
            Some(y) => (egui::Align2::CENTER_CENTER, pos2(area.center().x, y)),
            None => (
                egui::Align2::CENTER_TOP,
                pos2(area.center().x, area.top() + 8.0),
            ),
        };
        let shown = egui::Area::new(egui::Id::unique("reader-back-bar"))
            .order(egui::Order::Middle)
            .pivot(pivot)
            .fixed_pos(at)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(th.bar)
                    .stroke(egui::Stroke::new(1.0, th.muted.gamma_multiply(0.5)))
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let text = match &label {
                                Some(l) => format!("Back to where you were ({l})"),
                                None => "Back to where you were".to_owned(),
                            };
                            ui.visuals_mut().override_text_color = Some(th.text);
                            back = ui
                                .button(icons::text(ctx, icons::ARROW_LEFT, &text, 14.0))
                                .on_hover_text("Your reading place, time and stats didn't move (Esc)")
                                .clicked();
                            keep = ui
                                .button(RichText::new("Keep reading here").color(th.text))
                                .on_hover_text("Make this your reading place (counts as a jump: no characters)")
                                .clicked();
                            find = ui
                                .button(icons::text(ctx, icons::MAGNIFYING_GLASS, "", 14.0))
                                .on_hover_text("The search results (Ctrl+F)")
                                .clicked();
                        });
                    });
            });
        #[cfg(test)]
        {
            self.probe.back_bar = Some(shown.response.rect);
        }
        #[cfg(not(test))]
        let _ = shown;
        if back {
            self.peek_back();
        } else if keep {
            self.peek_keep();
        }
        if find {
            self.search.show();
        }
    }

    /// The app's known words (the popup's ✓ buttons), each frame.
    pub fn set_known(
        &mut self,
        known: Option<Arc<hanstat::anki::Knowledge>>,
        marked: Arc<std::collections::HashSet<String>>,
    ) {
        self.dict.set_known(known, marked);
    }

    /// Words marked known (true) or unmarked in the popup since last asked.
    pub fn take_known_toggles(&mut self) -> Vec<(String, bool)> {
        self.dict.take_known_toggles()
    }

    /// The notice's button was clicked (since last asked).
    pub fn take_notice_click(&mut self) -> bool {
        std::mem::take(&mut self.notice_clicked)
    }

    fn next(&mut self, g: &Geometry) {
        let Some(l) = self.layouts.get(&self.spine) else {
            return;
        };
        self.forward = true;
        self.turned_at = self.now;
        self.tracker.turned();
        let step = self.step(g);
        if self.page + step < l.pages.len() {
            self.page += step;
            self.offset = l.page_offset(self.page);
            self.sync_position(false);
            return;
        }
        if self.spine + 1 < self.epub.spine.len() {
            self.go_chapter(self.spine + 1, Target::Offset(0));
            if self.peek.is_none() {
                self.sync_position(true);
            }
        } else if self.peek.is_none() {
            self.stats_finish();
        }
    }

    fn prev(&mut self, g: &Geometry) {
        let Some(l) = self.layouts.get(&self.spine) else {
            return;
        };
        self.forward = false;
        self.turned_at = self.now;
        self.tracker.turned();
        let step = self.step(g);
        if self.page >= step {
            self.page -= step;
            self.offset = l.page_offset(self.page);
            self.sync_position(false);
            return;
        }
        if self.spine > 0 {
            self.go_chapter(self.spine - 1, Target::Last);
            if self.peek.is_none() {
                self.position_needs_save = true;
            }
        }
    }

    fn follow(&mut self, path: &str, fragment: Option<String>) {
        if let Some(i) = self.epub.spine.iter().position(|p| p == path) {
            self.forward = true;
            let target = fragment.map_or(Target::Offset(0), Target::Anchor);
            let is_offset = matches!(target, Target::Offset(_));
            self.go_chapter(i, target);
            if self.peek.is_none() {
                if is_offset {
                    self.sync_position(true);
                } else {
                    self.position_needs_save = true;
                }
            }
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
        if self.chapters_built.load(Ordering::Acquire) < self.epub.spine.len() {
            return None;
        }
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
        if let Some((before, lens, total)) = &self.text_totals {
            let here = lens.get(self.spine).map_or(0, |&n| self.offset.min(n));
            return (*total > 0).then(|| (before[self.spine] + here) as f32 / *total as f32);
        }
        if self.chapters_built.load(Ordering::Acquire) < self.epub.spine.len() {
            return None;
        }
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

    fn selected_text(&self) -> Option<String> {
        let (range, ch) = (self.selection.as_ref()?, self.chapter(self.spine)?);
        Some(
            ch.chars[range.start as usize..range.end as usize]
                .iter()
                .map(|&c| if c == '\0' { '\n' } else { c })
                .collect(),
        )
    }

    fn copy_selection(&self, ctx: &egui::Context) {
        if let Some(text) = self.selected_text() {
            ctx.copy_text(text);
        }
    }

    fn show_text_menu(
        &mut self,
        response: &egui::Response,
        id: egui::Id,
        change: Option<egui::SetOpenCommand>,
    ) {
        let Some(menu) = self.text_menu else { return };
        let has_selection = self.selection.is_some();
        let (play, export_quote) = egui::Popup::menu(response)
            .id(id)
            .open_memory(change)
            .at_position(menu.position)
            .show(|ui| {
                let copy = ui.add_enabled(has_selection, egui::Button::new("Copy"));
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
                let mut export = false;
                if has_selection {
                    ui.separator();
                    let btn =
                        ui.button(icons::text(ui.ctx(), icons::IMAGE, "Export as PNG…", 14.0));
                    if btn.clicked() {
                        export = true;
                        ui.close();
                    }
                }
                (play.clicked(), export)
            })
            .map_or((false, false), |menu| menu.inner);
        if play && let Some(spot) = menu.target {
            self.peek_keep();
            self.tts.play_from(spot.spine, spot.src, &self.settings.tts);
        }
        if export_quote && let Some(text) = self.selected_text() {
            let direction = self
                .settings
                .quote_direction
                .unwrap_or_else(|| quote::TextDirection::from_vertical(self.settings.vertical));
            self.quote_export = Some(quote::QuoteDialog::new(
                text,
                self.epub.title.clone(),
                self.epub.author.clone(),
                self.settings.quote_layout,
                direction,
                self.settings.quote_theme.clone(),
                self.settings.quote_font.clone(),
                self.settings.quote_title_font.clone(),
            ));
        }
    }

    /// Draw the reader. `false` once it is closed.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        use std::time::{Duration, Instant};
        let frame_start = Instant::now();
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        self.now = now;
        let th = theme::get(&ctx);
        self.sync_chapter_totals();
        self.images.next_frame();
        self.images.poll(&ctx);
        let mut open = true;
        self.dict.warm(&ctx);
        self.dict.size = self.settings.popup_size.map(|[w, h]| vec2(w, h));
        self.dict.theme.clone_from(&self.settings.popup_theme);
        self.dict.poll();
        self.dict.anchor = None;
        self.dicts.poll(&ctx, &self.dict);
        self.tts.poll(&self.settings.tts);
        // Anki cards' audio is said by read aloud's voice: the key is looked
        // for once cards with audio are set up, the voice handed over while
        // the popup is open (its + sends it with the note to make).
        if self.dict.cards.shown() && self.dict.cards.cfg.ready() && self.dict.cards.wants_audio() {
            if self.settings.tts.source == tts::Source::Azure {
                self.tts.prepare_key();
            }
            if self.dict.is_open() {
                self.dict.cards.voice = self.tts.card_voice(&self.settings.tts);
            }
        }
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
            || self.dict.cards.setup_open
            || self.quote_export.is_some();
        // A search result shown isn't reading: its time doesn't count.
        self.stats_frame(&ctx, menus_open);
        // Ctrl+F, wherever the keys are (the search box too).
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::F)) {
            self.search.show();
        }
        self.search.poll(now, &ctx);
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
            if self.quote_export.is_some() {
                self.quote_export = None;
            } else if self.dict.cards.setup_open {
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

        let bar_frame = egui::Frame::new()
            .fill(th.bar)
            .inner_margin(egui::Margin::symmetric(10, 6));
        #[cfg(test)]
        {
            self.probe.aa = None;
            self.probe.bar = None;
            self.probe.back_bar = None;
        }
        if self.settings.autohide_bar {
            // Over the pages, so they stay where they are whether it shows
            // or not. The bar's panels and windows keep it shown (the
            // read-aloud windows are drawn from it); the text menu doesn't.
            let screen = ui.max_rect();
            let bar_menus = menus_open
                && (!text_menu_open
                    || self.settings_open
                    || self.stats_open
                    || self.tts.panel_open
                    || self.tts.local_window_open()
                    || self.tts.design_window_open()
                    || self.dicts.open
                    || self.dict.cards.setup_open);
            let shown = self.bar_wanted(&ctx, screen, bar_menus);
            let t = ctx.animate_bool_with_time(egui::Id::unique("reader-bar-shown"), shown, 0.12);
            if t > 0.0 {
                let r = egui::Area::new(egui::Id::unique("reader-bar-overlay"))
                    .order(egui::Order::Middle)
                    .fixed_pos(screen.min)
                    .constrain(false)
                    .interactable(shown)
                    .show(&ctx, |ui| {
                        ui.multiply_opacity(t);
                        bar_frame
                            .show(ui, |ui| {
                                ui.set_width(screen.width() - bar_frame.inner_margin.sum().x);
                                self.bar(ui, th)
                            })
                            .inner
                    });
                self.bar_h = r.response.rect.height();
                #[cfg(test)]
                {
                    self.probe.bar = Some(r.response.rect);
                }
                (open, play_page) = r.inner;
            }
        } else {
            let r = egui::Panel::top("reader-bar")
                .frame(bar_frame)
                .show(ui, |ui| self.bar(ui, th));
            #[cfg(test)]
            {
                self.probe.bar = Some(r.response.rect);
            }
            (open, play_page) = r.inner;
        }

        egui::CentralPanel::no_frame().show(ui, |ui| {
            let area = ui.max_rect();
            self.page_area = Some(area);
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
                vertical: self.settings.vertical,
            };
            // Not while a slider or the window edge is being dragged (toggling
            // horizontal/vertical switches immediately).
            let mode_flipped = self
                .params
                .as_ref()
                .is_some_and(|p| p.vertical != params.vertical);
            let settled = mode_flipped
                || (now - self.resized_at > 0.15
                    && !text_input_busy
                    && !ctx.input(|i| i.pointer.any_down()))
                || self.layouts.is_empty();
            if self.params.as_ref() != Some(&params) {
                if settled {
                    self.params = Some(params);
                    self.invalidate_layouts();
                    self.drawn = Some(g);
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(160));
                }
            } else {
                if self.drawn.is_some_and(|d| d.spread != g.spread) && self.target.is_none() {
                    self.target = Some(self.reflow_target());
                }
                self.drawn = Some(g);
            }
            let g = self.drawn.unwrap_or(g);
            geom_for_keys = Some(g);
            let Some(l) = self.layout(&ctx, self.spine, Instant::now() + Duration::from_millis(4)) else {
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
                        if self.peek.is_none() {
                            self.position_needs_save = true;
                        }
                    } else if !self.forward && self.spine > 0 {
                        self.go_chapter(self.spine - 1, Target::Last);
                        if self.peek.is_none() {
                            self.position_needs_save = true;
                        }
                    }
                    ctx.request_repaint();
                    return;
                }
                self.page = match &t {
                    Target::Offset(o) => l.page_of_offset(*o),
                    Target::ReflowSrc { offset, src } => l
                        .page_of_src(*src)
                        .unwrap_or_else(|| l.page_of_offset(*offset)),
                    Target::Src(s) => l.page_of_src(*s).unwrap_or(l.pages.len() - 1),
                    Target::Anchor(a) => l.page_of_anchor(a).unwrap_or(0),
                    Target::Last => l.pages.len() - 1,
                };
                self.page -= self.page % step;
                self.offset = match t {
                    Target::Offset(o) | Target::ReflowSrc { offset: o, .. } => {
                        o.min(l.text_total)
                    }
                    Target::Src(_) | Target::Anchor(_) | Target::Last => l.page_offset(self.page),
                };
                if self.peek.is_none() {
                    let flush = std::mem::take(&mut self.position_needs_save);
                    self.sync_position(flush);
                }
            }
            self.page = self.page.min(l.pages.len().saturating_sub(1));
            self.page -= self.page % step;
            self.stats_place(&l);
            self.tts.page_src = l.page_first_src(self.page).unwrap_or(0);
            self.tts.page_end = l.page_first_src(self.page + step).unwrap_or(u32::MAX);
            if play_page {
                self.peek_keep();
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
            let spoken_chapter = if self.peek.is_none()
                && !text_input_busy
                && !resp.is_pointer_button_down_on()
                && !primary_click
                && !secondary_click
            {
                self.follow_reading(&l, step, &ctx)
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
                        if l.vertical {
                            off.x -= ((g.content.x - l.page_height(idx)) / 2.0).max(0.0);
                        } else {
                            off.y += ((g.content.y - l.page_height(idx)) / 2.0).max(0.0);
                        }
                    }
                    let off = off + l.page_shift(idx);
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
            let hit_spoiler = |p: Pos2| {
                sides.iter().find_map(|&(idx, off, clip)| {
                    clip.contains(p)
                        .then(|| {
                            l.page_spoilers(idx)
                                .find(|s| s.rect.translate(off).contains(p))
                                .map(|s| s.id)
                        })
                        .flatten()
                })
            };
            let spine = self.spine;
            let primary = egui::PointerButton::Primary;
            if !popup_was_open
                && resp.is_pointer_button_down_on()
                && let Some(p) = ctx.input(|i| {
                    i.pointer
                        .button_pressed(primary)
                        .then(|| i.pointer.press_origin())
                        .flatten()
                })
                && !hit_spoiler(p).is_some_and(|id| !self.revealed_spoilers.contains(&(spine, id)))
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
            let had_selection = self.selection.is_some();
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
                    (hit_text(p).is_some()
                        && !hit_spoiler(p)
                            .is_some_and(|id| !self.revealed_spoilers.contains(&(spine, id))))
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
                    let sentences = tts::sentences_of(&ch);
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
            let mut hovered_blurred_spoiler: Option<u32> = None;
            if !popup_was_open && menu_position.is_none() {
                if dragged && self.selection_anchor.is_some() {
                    ctx.set_cursor_icon(egui::CursorIcon::Text);
                } else if let Some(p) = resp.hover_pos() {
                    if hit_link(p).is_some() {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    } else if let Some(id) = hit_spoiler(p)
                        .filter(|&id| !self.revealed_spoilers.contains(&(spine, id)))
                    {
                        hovered_blurred_spoiler = Some(id);
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
                && !hit_spoiler(h).is_some_and(|id| !self.revealed_spoilers.contains(&(spine, id)))
                && let Some((src, _, _)) = hit_text(h)
                && let Some(ch) = self.chapter(self.spine)
            {
                let spot = popup::Spot {
                    spine: self.spine,
                    src,
                };
                self.dict.point(spot, &ch.chars, lookup_key, now, &ctx);
            }
            #[cfg(test)]
            {
                self.probe.sides = sides.iter().map(|(i, o, _)| (*i, *o)).collect();
            }
            let highlight = self.dict.highlight(self.spine);
            let stats_line = self.stats_line(crate::reading_stats::now());
            for side in 0..step {
                let page_rect = g.pages[side];
                draw_page(ui, th, page_rect, side, g.spread);
                let idx = self.page + side;
                let Some(&(_, off, clip)) = sides.iter().find(|s| s.0 == idx) else {
                    continue;
                };
                let items = l.page_render_items(idx);
                let clip_pad = if l.vertical {
                    vec2(0.0, g.inset.y * 0.4)
                } else {
                    vec2(g.inset.x * 0.5, 0.0)
                };
                let painter = ui.painter_at(clip.expand2(clip_pad));
                let ppp = ctx.pixels_per_point();
                let hl_pad = if l.vertical {
                    vec2(0.0, 1.0)
                } else {
                    vec2(1.0, 0.0)
                };
                if let Some(r) = &reading {
                    for r in l.char_rects(idx, r.clone()) {
                        painter.rect_filled(
                            r.translate(off).expand2(hl_pad),
                            3.0,
                            th.reading(),
                        );
                    }
                }
                if let Some((src, n)) = highlight {
                    for r in l.char_rects(idx, src..src + n as u32) {
                        let r = r.translate(off);
                        painter.rect_filled(r.expand2(hl_pad), 3.0, th.highlight());
                        self.dict.anchor = Some(self.dict.anchor.map_or(r, |a| a.union(r)));
                    }
                }
                for (i, spoiler) in items {
                    if spoiler.is_some_and(|id| !self.revealed_spoilers.contains(&(spine, id))) {
                        continue;
                    }
                    paint_item(&mut self.images, th, &painter, &l.items[i], off, ppp);
                }
                for s in l.page_spoilers(idx) {
                    if !self.revealed_spoilers.contains(&(spine, s.id)) {
                        paint_spoiler_blur(
                            &painter,
                            th,
                            s,
                            off,
                            l.vertical,
                            hovered_blurred_spoiler == Some(s.id),
                        );
                    }
                }
                if let Some((sp, range)) = &self.search_hit
                    && *sp == self.spine
                {
                    for r in l.char_rects(idx, range.clone()) {
                        painter.rect_filled(r.translate(off).expand2(hl_pad), 3.0, th.search_hit());
                    }
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
                    "Page {} of {} in this chapter (pages change with the window, page size, text size and margins)",
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
                } else if let Some(id) = hit_spoiler(p) {
                    let key = (self.spine, id);
                    if !self.revealed_spoilers.contains(&key) {
                        self.revealed_spoilers.insert(key);
                    } else if !had_selection {
                        self.revealed_spoilers.remove(&key);
                    }
                } else if let Some((src, _, _)) = hit_text(p) {
                    if self.tts.active() {
                        self.peek_keep();
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
                        if self.settings.vertical {
                            self.next(&g);
                        } else {
                            self.prev(&g);
                        }
                    } else if p.x > right {
                        if self.settings.vertical {
                            self.prev(&g);
                        } else {
                            self.next(&g);
                        }
                    }
                }
            }
            if let Some((spine, src)) = spoken_chapter {
                self.forward = spine > self.spine;
                self.tracker.turned();
                self.go_chapter(spine, Target::Src(src));
                if self.peek.is_none() {
                    self.position_needs_save = true;
                }
                ctx.request_repaint();
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
                // A: the first entry's word known (or not).
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::A)) {
                    self.dict.toggle_first_known();
                }
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::P)) {
                self.stats_toggle();
            }
            // Read aloud: Ctrl+← → by sentences, K pauses (as video players do;
            // Ctrl+Space switches Chinese input methods).
            if self.tts.active() {
                let (back, fwd, pause) = ctx.input_mut(|i| {
                    (
                        i.consume_key(egui::Modifiers::COMMAND, Key::ArrowLeft),
                        i.consume_key(egui::Modifiers::COMMAND, Key::ArrowRight),
                        i.consume_key(egui::Modifiers::NONE, Key::K),
                    )
                });
                if back {
                    self.tts.step(-1);
                }
                if fwd {
                    self.tts.step(1);
                }
                if pause {
                    self.tts.toggle_pause();
                }
            }
            let (fwd_arrow, back_arrow) = if self.settings.vertical {
                (Key::ArrowLeft, Key::ArrowRight)
            } else {
                (Key::ArrowRight, Key::ArrowLeft)
            };
            let (fwd, back, home, end, esc, wheel) = ctx.input_mut(|i| {
                let fwd = [fwd_arrow, Key::ArrowDown, Key::PageDown, Key::Space]
                    .iter()
                    .any(|k| i.consume_key(egui::Modifiers::NONE, *k));
                let back = [back_arrow, Key::ArrowUp, Key::PageUp, Key::Backspace]
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
            let over_search = ctx
                .pointer_hover_pos()
                .is_some_and(|p| self.search.rect.is_some_and(|r| r.contains(p)));
            let wheel = if over_dict || over_search { 0.0 } else { wheel };
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
                self.turned_at = self.now;
                self.tracker.jumped();
                self.target = None;
                self.page = 0;
                self.offset = 0;
                self.sync_position(true);
            }
            if end
                && let Some(l) = self.layouts.get(&self.spine)
                && !l.pages.is_empty()
            {
                let step = self.step(&g);
                self.turned_at = self.now;
                self.tracker.jumped();
                self.target = None;
                let last = l.pages.len() - 1;
                self.page = last - last % step;
                self.offset = l.page_offset(self.page);
                self.sync_position(true);
            }
            if esc && self.search.open {
                // Esc with the sidebar shown closes it, not the book.
                self.search.close();
            } else if esc && self.peek.is_some() {
                // Esc on a search result: back to reading, not out of the book.
                self.peek_back();
            } else if esc {
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
        self.search_frame(&ctx, th);
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
        if let Some(mut dlg) = self.quote_export.take() {
            let before_layout = dlg.card.layout;
            let before_direction = dlg.card.direction;
            let before_theme = dlg.theme_id.clone();
            let before_quote_font = dlg.quote_font.clone();
            let before_title_font = dlg.title_font.clone();
            let (mut keep_open, save) =
                dlg.ui(&ctx, th, self.fonts.as_ref(), self.settings.sans_default);
            if dlg.card.layout != before_layout {
                self.settings.quote_layout = dlg.card.layout;
                self.dirty = true;
            }
            if dlg.card.direction != before_direction {
                self.settings.quote_direction = Some(dlg.card.direction);
                self.dirty = true;
            }
            if dlg.theme_id != before_theme {
                self.settings.quote_theme = dlg.theme_id.clone();
                self.dirty = true;
            }
            if dlg.quote_font != before_quote_font {
                self.settings.quote_font = dlg.quote_font.clone();
                self.dirty = true;
            }
            if dlg.title_font != before_title_font {
                self.settings.quote_title_font = dlg.title_font.clone();
                self.dirty = true;
            }
            if save
                && let Some(path) = rfd::FileDialog::new()
                    .set_file_name(quote::suggested_filename(&dlg.card.title))
                    .add_filter("PNG", &["png"])
                    .save_file()
            {
                let export_th = dlg.resolved_theme(th);
                let fams = fonts::families();
                match quote::export_png(
                    &ctx,
                    &dlg.card,
                    export_th,
                    self.fonts.as_ref(),
                    self.settings.sans_default,
                    dlg.quote_font.as_deref(),
                    dlg.title_font.as_deref(),
                    fams.as_deref().map(Vec::as_slice),
                    &path,
                ) {
                    Ok(()) => keep_open = false,
                    Err(e) => dlg.error = Some(e),
                }
            }
            if keep_open {
                self.quote_export = Some(dlg);
            }
        }
        if self.dirty && !ctx.input(|i| i.pointer.any_down()) {
            self.save();
        } else if let Some(at) = self.position_dirty_at {
            let elapsed = now - at;
            if elapsed >= 2.0 && !ctx.input(|i| i.pointer.any_down()) {
                self.sync_position(true);
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64((2.0 - elapsed).max(0.05)));
            }
        }
        if let Some(e) = self.error.take() {
            crate::palette::popup_window(&ctx, "Reader")
                .collapsible(false)
                .show(&ctx, |ui| ui.label(e));
        }
        self.goal_met_effects(&ctx);

        // Navigation and popups take their turn before speculative pagination.
        // If the reader moves into this chapter, layout continues from here.
        let next = self.spine + 1;
        if open
            && self.target.is_none()
            && next < self.epub.spine.len()
            && self.layouts.contains_key(&self.spine)
            && !self.layouts.contains_key(&next)
        {
            let wait = 0.5 - (now - self.turned_at);
            let deadline = frame_start + Duration::from_millis(6);
            if wait > 0.0 {
                ctx.request_repaint_after(Duration::from_secs_f64(wait));
            } else if menus_open
                || self.dict.is_open()
                || ctx.input(|i| !i.events.is_empty())
                || Instant::now() >= deadline
            {
                ctx.request_repaint_after(Duration::from_millis(50));
            } else {
                let _ = self.layout(&ctx, next, deadline);
            }
        }
        open
    }

    /// Show the hidden menu bar ([`Settings::autohide_bar`]) this frame:
    /// with the pointer where it is (not while a button is held, so a
    /// selection dragged up doesn't bring it out), while `menus` (its panels)
    /// are open, and for a while after either, or after a new notice.
    fn bar_wanted(&mut self, ctx: &egui::Context, screen: Rect, menus: bool) -> bool {
        const LINGER: f64 = 0.5;
        let now = self.now;
        let shown = now < self.bar_until;
        match &self.notice {
            Some(n) if self.bar_notice.as_deref() != Some(n.text.as_str()) => {
                self.bar_notice = Some(n.text.clone());
                self.bar_until = self.bar_until.max(now + 4.0);
            }
            Some(_) => {}
            None => self.bar_notice = None,
        }
        let (pointer, down) = ctx.input(|i| (i.pointer.hover_pos(), i.pointer.any_down()));
        // Once out, a little below it too, so it doesn't flicker at its edge.
        let reach = self.bar_h + if shown { 8.0 } else { 0.0 };
        let at_top = pointer.is_some_and(|p| screen.contains(p) && p.y < screen.top() + reach);
        if menus || (at_top && (shown || !down)) {
            self.bar_until = self.bar_until.max(now + LINGER);
        }
        if now < self.bar_until {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                self.bar_until - now + 0.01,
            ));
            true
        } else {
            false
        }
    }

    /// The menu bar's contents: whether the reader stays open, and whether
    /// to read the page aloud.
    fn bar(&mut self, ui: &mut egui::Ui, th: &Theme) -> (bool, bool) {
        let mut open = true;
        let mut play_page = false;
        ui.horizontal(|ui| {
            if ui
                .button(icons::rich(
                    ui,
                    RichText::new(format!("{} Back", icons::CARET_LEFT)).size(15.0),
                ))
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
                let read = self.tts.bar(ui, &mut self.settings.tts);
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
                ui.add_enabled_ui(!self.settings.vertical, |ui| {
                    let spread_tip = if self.settings.vertical {
                        "Vertical mode uses one landscape page"
                    } else {
                        "Two pages side by side, when the window is wide enough"
                    };
                    ui.selectable_value(
                        &mut self.settings.spread,
                        true,
                        RichText::new("Two pages").size(14.0),
                    )
                    .on_hover_text(spread_tip)
                    .on_disabled_hover_text("Vertical mode uses one landscape page");
                    ui.selectable_value(
                        &mut self.settings.spread,
                        false,
                        RichText::new("One page").size(14.0),
                    )
                    .on_disabled_hover_text("Vertical mode uses one landscape page");
                });
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
        (open, play_page)
    }

    /// Turn to the sentence read aloud when it leaves the pages shown, if
    /// the one before was on them (pages turned away by hand stay), and
    /// within a sentence that goes on past them, when the voice reaches the
    /// next page's first character.
    /// If it moves to another chapter, return its target to open after painting.
    fn follow_reading(
        &mut self,
        l: &ChapterLayout,
        step: usize,
        ctx: &egui::Context,
    ) -> Option<(usize, u32)> {
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
            if visible && spine == self.spine {
                self.follow_within(l, step, &r, ctx);
            }
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
            self.sync_position(false);
        }
        None
    }

    /// The sentence heard is on the pages shown and goes on past them: turn
    /// when the voice reaches the next page, estimated from how far into
    /// the sentence's sound it is (as many characters as seconds in).
    fn follow_within(
        &mut self,
        l: &ChapterLayout,
        step: usize,
        r: &std::ops::Range<u32>,
        ctx: &egui::Context,
    ) {
        let next = self.page + step;
        let Some(end) = l.page_first_src(next) else {
            return;
        };
        if end <= r.start || end >= r.end {
            return;
        }
        let Some(clock) = self.tts.clock() else {
            return;
        };
        let frac = (end - r.start) as f32 / (r.end - r.start) as f32;
        let when = clock.at(frac);
        let now = std::time::Instant::now();
        if now < when {
            ctx.request_repaint_after(when - now);
            return;
        }
        self.turned_at = self.now;
        self.forward = true;
        self.tracker.turned();
        self.page = next;
        self.offset = l.page_offset(self.page);
        self.sync_position(false);
    }

    fn contents_menu(&mut self, ui: &mut egui::Ui) {
        if self.epub.toc.is_empty() {
            return;
        }
        let mut go = None;
        let scroll = !self.contents_shown;
        #[cfg(test)]
        {
            self.probe.toc_item = None;
        }
        #[cfg(test)]
        let mut toc_item = None;
        let menu = ui.menu_button(
            icons::rich(
                ui,
                RichText::new(format!("{} Contents", icons::LIST)).size(15.0),
            ),
            |ui| {
                // Each entry is one line of text: its name, then its length in a
                // column just past the longest name, so the menu is as wide as its
                // contents (a right-aligned column would stretch it across the window).
                let font = egui::FontId::proportional(15.0);
                let small = egui::FontId::proportional(13.0);
                let indent = |e: &epub::TocEntry| e.depth as f32 * 16.0;
                let current = self.toc_current();
                let need_lengths = self.toc_cache.as_ref().is_none_or(|c| !c.has_lengths);
                let lengths = if need_lengths {
                    self.toc_lengths()
                } else {
                    None
                };
                let toc = &self.epub.toc;
                let labels = &self.toc_labels;
                let cache = self.toc_cache.get_or_insert_with(|| {
                    let (name_w, ellipsis_w) = ui.fonts_mut(|f| {
                        let nw: Vec<f32> = labels
                            .iter()
                            .map(|l| {
                                f.layout_no_wrap(l.clone(), font.clone(), Color32::WHITE)
                                    .size()
                                    .x
                            })
                            .collect();
                        let ew = f
                            .layout_no_wrap("…".to_owned(), small.clone(), Color32::WHITE)
                            .size()
                            .x;
                        (nw, ew)
                    });
                    let names_col = toc
                        .iter()
                        .zip(&name_w)
                        .map(|(e, w)| indent(e) + w)
                        .fold(0.0, f32::max)
                        .min(480.0);
                    TocCache {
                        has_lengths: false,
                        name_w,
                        numbers: Vec::new(),
                        number_w: Vec::new(),
                        ellipsis_w,
                        names_col,
                        numbers_col: ellipsis_w,
                    }
                });
                if !cache.has_lengths
                    && let Some(lengths) = lengths
                {
                    let numbers: Vec<String> =
                        lengths.iter().map(|&l| crate::fmt_int(l as u64)).collect();
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
                    cache.numbers_col = number_w.iter().copied().fold(0.0, f32::max);
                    cache.numbers = numbers;
                    cache.number_w = number_w;
                    cache.has_lengths = true;
                }
                let text = ui.visuals().text_color();
                let muted = theme::get(ui.ctx()).muted;
                let max = crate::palette::dropdown_max_size(ui.ctx(), 72.0);
                let def_w =
                    (cache.names_col + cache.numbers_col + 56.0).clamp(200.0_f32.min(max.x), max.x);
                let def_h =
                    (ui.ctx().content_rect().height() * 0.7).clamp(120.0_f32.min(max.y), max.y);
                let min_s = vec2(200.0_f32.min(max.x), 120.0_f32.min(max.y));
                let row_h = (ui.fonts_mut(|f| f.row_height(&font))
                    + 2.0 * ui.spacing().button_padding.y)
                    .max(ui.spacing().interact_size.y);
                egui::Resize::default()
                    .id_salt("reader-contents-resize")
                    .default_size(vec2(def_w, def_h))
                    .min_size(min_s)
                    .max_size(max)
                    .show(ui, |ui| {
                        let visible_max_h = ui.available_height();
                        let mut scroll_area = egui::ScrollArea::both()
                            .id_salt("reader-contents-scroll")
                            .auto_shrink([false, true]);
                        if scroll && let Some(cur) = current {
                            let spacing = ui.spacing().item_spacing.y;
                            let stride = row_h + spacing;
                            let content_h = (toc.len() as f32 * stride - spacing).max(0.0);
                            let visible_h = visible_max_h.min(content_h);
                            let target_y = (cur as f32 * stride + row_h * 0.5 - visible_h * 0.5)
                                .clamp(0.0, (content_h - visible_h).max(0.0));
                            scroll_area = scroll_area.vertical_scroll_offset(target_y);
                        }
                        scroll_area.show_rows(ui, row_h, toc.len(), |ui, rows| {
                            for i in rows {
                                let e = &toc[i];
                                let ind = indent(e);
                                let (num_str, num_w) = if cache.has_lengths {
                                    (cache.numbers[i].as_str(), cache.number_w[i])
                                } else {
                                    ("…", cache.ellipsis_w)
                                };
                                let mut job = egui::text::LayoutJob::default();
                                job.append(
                                    &labels[i],
                                    ind,
                                    egui::TextFormat::simple(font.clone(), text),
                                );
                                // Gap to the numbers' column, numbers right-aligned in it.
                                let gap = (cache.names_col - ind - cache.name_w[i]).max(0.0)
                                    + 28.0
                                    + (cache.numbers_col - num_w);
                                job.append(
                                    num_str,
                                    gap,
                                    egui::TextFormat {
                                        valign: egui::Align::Center,
                                        ..egui::TextFormat::simple(small.clone(), muted)
                                    },
                                );
                                let r = ui.selectable_label(current == Some(i), job).on_hover_text(
                                    "Its length: Chinese characters, letters and digits",
                                );
                                #[cfg(test)]
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
            },
        );
        #[cfg(test)]
        {
            self.probe.contents = Some(menu.response.rect);
            self.probe.toc_item = toc_item;
        }
        self.contents_shown = menu.inner.is_some();
        if let Some(i) = go {
            let e = self.epub.toc[i].clone();
            self.forward = true;
            let target = e.fragment.map_or(Target::Offset(0), Target::Anchor);
            let is_offset = matches!(target, Target::Offset(_));
            self.go_chapter(e.spine, target);
            if self.peek.is_none() {
                if is_offset {
                    self.sync_position(true);
                } else {
                    self.position_needs_save = true;
                }
            }
        }
    }

    fn settings_menu(&mut self, ui: &mut egui::Ui) {
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
        #[cfg(test)]
        {
            self.probe.aa = Some(button.rect);
            self.probe.theme_box = None;
            self.probe.theme_items.clear();
            self.probe.popup_theme_box = None;
            self.probe.popup_same = None;
            self.probe.popup_theme_items.clear();
            self.probe.serif_box = None;
            self.probe.serif_item = None;
            self.probe.vertical = None;
        }
        let mut changed = false;
        let mut open = self.settings_open;
        egui::Popup::menu(&button)
            .id(egui::Id::unique("reader-settings"))
            .open_bool(&mut open)
            .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
            .show(|ui| {
            let max = crate::palette::dropdown_max_size(ui.ctx(), 72.0);
            crate::palette::resizable_scroll_popup(
                ui,
                "reader-settings-box",
                vec2(400.0, 440.0),
                vec2(260.0, 160.0),
                max,
                |ui| {
            let before = (
                self.settings.theme.clone(),
                self.settings.popup_theme.clone(),
                self.settings.serif.clone(),
                self.settings.sans.clone(),
                self.settings.sans_default,
                self.settings.size,
                self.settings.line_spacing,
                self.settings.margins,
                self.settings.vertical,
                self.settings.spread,
                self.settings.autohide_bar,
            );
            let before_page = (self.settings.page_width, self.settings.page_height);
            let fams = fonts::families();
            egui::Grid::new("reader-settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Theme");
                let current = theme::index(&self.settings.theme);
                let mut chosen = None;
                #[cfg(test)]
                let mut items = Vec::new();
                let button = ui.add(
                    egui::Button::new(theme::THEMES[current].name)
                        .right_text(icons::glyph(ui.ctx(), icons::CARET_DOWN, false))
                        .min_size(vec2(220.0, 0.0)),
                );
                egui::Popup::menu(&button)
                    .id(egui::Id::unique("reader-theme-list"))
                    .show(|ui| {
                        let max = crate::palette::dropdown_max_size(ui.ctx(), 72.0);
                        let w = 2.0 * THEME_COLUMN + ui.spacing().item_spacing.x + 12.0;
                        crate::palette::resizable_scroll_popup(
                            ui,
                            "reader-theme-list-box",
                            vec2(w, 460.0),
                            vec2(220.0, 140.0),
                            max,
                            |ui| {
                                #[cfg(test)]
                                {
                                    chosen = theme_columns(ui, Some(current), &mut items);
                                }
                                #[cfg(not(test))]
                                {
                                    chosen = theme_columns(ui, Some(current));
                                }
                            },
                        );
                    });
                #[cfg(test)]
                {
                    self.probe.theme_box = Some(button.rect);
                    self.probe.theme_items = items;
                }
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
                #[cfg(test)]
                let mut same = None;
                #[cfg(test)]
                let mut items = Vec::new();
                let button = ui
                    .add(
                        egui::Button::new(current.map_or("Same as pages", |i| theme::THEMES[i].name))
                            .right_text(icons::glyph(ui.ctx(), icons::CARET_DOWN, false))
                            .min_size(vec2(220.0, 0.0)),
                    )
                    .on_hover_text("The dictionary popup's colours");
                egui::Popup::menu(&button)
                    .id(egui::Id::unique("reader-popup-theme-list"))
                    .show(|ui| {
                        let max = crate::palette::dropdown_max_size(ui.ctx(), 72.0);
                        let w = 2.0 * THEME_COLUMN + ui.spacing().item_spacing.x;
                        crate::palette::resizable_scroll_popup(
                            ui,
                            "reader-popup-theme-list-box",
                            vec2(w + 12.0, 490.0),
                            vec2(220.0, 140.0),
                            max,
                            |ui| {
                                // As wide as the two columns below it, in the pages' colours.
                                let r = ui
                                    .allocate_ui(vec2(w, 30.0), |ui| {
                                        theme::swatch_as(ui, &theme::THEMES[pages], "Same as pages", current.is_none())
                                    })
                                    .inner;
                                if r.clicked() {
                                    chosen = Some(popup::SAME_AS_PAGES);
                                    ui.close();
                                }
                                #[cfg(test)]
                                {
                                    same = Some(r.rect);
                                    if let Some(i) = theme_columns(ui, current, &mut items) {
                                        chosen = Some(theme::THEMES[i].id);
                                    }
                                }
                                #[cfg(not(test))]
                                if let Some(i) = theme_columns(ui, current) {
                                    chosen = Some(theme::THEMES[i].id);
                                }
                            },
                        );
                    });
                #[cfg(test)]
                {
                    self.probe.popup_theme_box = Some(button.rect);
                    self.probe.popup_same = same;
                    self.probe.popup_theme_items = items;
                }
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
                    #[cfg(test)]
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
                                        #[cfg(test)]
                                        if i == 2 {
                                            item = Some((r.rect, fam.name.clone()));
                                        }
                                        #[cfg(not(test))]
                                        let _ = i;
                                    }
                                }
                            }
                        });
                    #[cfg(test)]
                    if serif {
                        self.probe.serif_box = Some(combo.response.rect);
                        self.probe.serif_item = item;
                    }
                    #[cfg(not(test))]
                    let _ = combo;
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
                let area = self.page_area.unwrap_or_else(|| ui.ctx().content_rect());
                let bounds = page_bounds(&self.settings, area);
                page_dimension_row(
                    ui,
                    "Page width",
                    &mut self.settings.page_width,
                    bounds.default_w,
                    bounds.min_w,
                    bounds.max_w,
                    if bounds.spread {
                        "Width of each page in points (Auto follows the window and text size)"
                    } else {
                        "Width of the page in points (Auto follows the window and text size)"
                    },
                    "Use the default page width",
                );
                page_dimension_row(
                    ui,
                    "Page height",
                    &mut self.settings.page_height,
                    bounds.default_h,
                    bounds.min_h,
                    bounds.max_h,
                    "Height of the page in points (Auto follows the window and text size)",
                    "Use the default page height",
                );
                ui.label("Text direction");
                #[cfg(test)]
                let mut vert_rect = None;
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.settings.vertical, false, "Horizontal");
                    let v = ui.radio_value(&mut self.settings.vertical, true, "Vertical");
                    #[cfg(test)]
                    {
                        vert_rect = Some(v.rect);
                    }
                    #[cfg(not(test))]
                    let _ = v;
                })
                .response
                .on_hover_text("Vertical: top to bottom, right to left, on a landscape page");
                #[cfg(test)]
                {
                    self.probe.vertical = vert_rect;
                }
                ui.end_row();
                ui.label("Pages");
                ui.add_enabled_ui(!self.settings.vertical, |ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.settings.spread, false, "One");
                        ui.radio_value(&mut self.settings.spread, true, "Two side by side");
                    })
                    .response
                    .on_hover_text("Two pages when the window is wide enough")
                    .on_disabled_hover_text("Vertical mode uses one landscape page");
                });
                ui.end_row();
                ui.label("Menu bar");
                ui.checkbox(&mut self.settings.autohide_bar, "Hide until the pointer is at the top")
                    .on_hover_text("The bar then shows over the pages, which stay where they are");
                ui.end_row();
            });
            if (
                before.0.as_str(),
                before.1.as_str(),
                before.2.as_deref(),
                before.3.as_deref(),
                before.4,
                before.5,
                before.6,
                before.7,
                before.8,
                before.9,
                before.10,
            ) != (
                self.settings.theme.as_str(),
                self.settings.popup_theme.as_str(),
                self.settings.serif.as_deref(),
                self.settings.sans.as_deref(),
                self.settings.sans_default,
                self.settings.size,
                self.settings.line_spacing,
                self.settings.margins,
                self.settings.vertical,
                self.settings.spread,
                self.settings.autohide_bar,
            ) || before_page != (self.settings.page_width, self.settings.page_height)
            {
                changed = true;
            }
                },
            );
        });
        self.settings_open = open;
        if changed {
            self.dirty = true;
        }
    }
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

#[allow(clippy::too_many_arguments)]
fn page_dimension_row(
    ui: &mut egui::Ui,
    label: &str,
    setting: &mut Option<f32>,
    default: f32,
    min: f32,
    max: f32,
    hover: &str,
    reset_hover: &str,
) {
    ui.label(label);
    let lo = min.ceil();
    let hi = max.floor().max(lo);
    let mut val = setting.unwrap_or(default).clamp(lo, hi).round();
    let is_auto = setting.is_none();
    ui.horizontal(|ui| {
        let r = ui
            .add(
                egui::Slider::new(&mut val, lo..=hi)
                    .step_by(1.0)
                    .custom_formatter(move |v, _| {
                        if is_auto {
                            "Auto".to_owned()
                        } else {
                            format!("{v:.0} pt")
                        }
                    })
                    .custom_parser(|s| {
                        s.trim()
                            .strip_suffix("pt")
                            .unwrap_or(s)
                            .trim()
                            .parse::<f64>()
                            .ok()
                    }),
            )
            .on_hover_text(hover);
        if r.changed() {
            *setting = Some(val);
        }
        if ui
            .add_enabled(setting.is_some(), egui::Button::new("Reset"))
            .on_hover_text(reset_hover)
            .clicked()
        {
            *setting = None;
        }
    });
    ui.end_row();
}

/// Default and allowable outer page dimensions in `area`.
struct PageBounds {
    default_w: f32,
    default_h: f32,
    min_w: f32,
    max_w: f32,
    min_h: f32,
    max_h: f32,
    spread: bool,
    pad_x: f32,
    pad_top: f32,
    pad_bottom: f32,
    desk: f32,
}

fn page_bounds(settings: &Settings, area: Rect) -> PageBounds {
    let base = settings.size;
    let pad_x = (base * 2.2 * settings.margins).max(12.0);
    let pad_top = (base * 2.4 * settings.margins).max(24.0);
    let pad_bottom = (base * 2.4 * settings.margins).max(28.0);
    let desk = 18.0;
    if settings.vertical {
        let avail_w = (area.width() - 2.0 * desk).max(200.0);
        let avail_h = (area.height() - 2.0 * desk).max(200.0);
        let max_col_text = base * 32.0;
        let pad_v = pad_top.max(pad_bottom);
        let min_h = (base * 6.0 + 2.0 * pad_v).min(avail_h);
        // Single landscape page: comfortable column height, wider than tall
        // even in narrow or square windows.
        let default_h = avail_h
            .min(max_col_text + 2.0 * pad_v)
            .min((avail_w / 1.3).max(min_h))
            .max(min_h);
        let default_w = avail_w
            .min(default_h * 1.6)
            .max(default_h.min(avail_w))
            .max(160.0);
        let min_w = (base * 4.0 + 2.0 * pad_x).max(160.0).min(avail_w);
        return PageBounds {
            default_w,
            default_h,
            min_w,
            max_w: avail_w.max(min_w),
            min_h,
            max_h: avail_h.max(min_h),
            spread: false,
            pad_x,
            pad_top,
            pad_bottom,
            desk,
        };
    }
    let max_text = base * 34.0;
    let min_spread_text = base * 20.0;
    let avail_h = (area.height() - 2.0 * desk).max(200.0);
    let spread = settings.spread
        && area.width() - 3.0 * desk >= 2.0 * (min_spread_text + 2.0 * pad_x)
        && area.width() > area.height() * 1.15;
    let avail_page_w = if spread {
        (area.width() - 3.0 * desk) / 2.0
    } else {
        area.width() - 2.0 * desk
    }
    .max(160.0);
    let default_w = avail_page_w.min(max_text + 2.0 * pad_x).max(160.0);
    let min_w = (base * 4.0 + 2.0 * pad_x).max(160.0).min(avail_page_w);
    let min_h = (base * 4.0 + pad_top + pad_bottom).max(200.0).min(avail_h);
    PageBounds {
        default_w,
        default_h: avail_h,
        min_w,
        max_w: avail_page_w.max(min_w),
        min_h,
        max_h: avail_h.max(min_h),
        spread,
        pad_x,
        pad_top,
        pad_bottom,
        desk,
    }
}

/// Where the pages go in `area`.
fn geometry(settings: &Settings, area: Rect) -> Geometry {
    let base = settings.size;
    let b = page_bounds(settings, area);
    let page_w = settings
        .page_width
        .map_or(b.default_w, |w| w.clamp(b.min_w, b.max_w));
    let page_h = settings
        .page_height
        .map_or(b.default_h, |h| h.clamp(b.min_h, b.max_h));
    if settings.vertical {
        let pad_v = b.pad_top.max(b.pad_bottom);
        // Column height a whole number of characters: CJK columns then fill
        // it without stretching, and rows of characters line up.
        let text_h = ((page_h - 2.0 * pad_v) / base).floor().max(4.0) * base;
        let inset_y = (page_h - text_h) / 2.0;
        let content_w = (page_w - 2.0 * b.pad_x).max(base * 3.0);
        let x0 = area.center().x - page_w / 2.0;
        let y0 = area.center().y - page_h / 2.0;
        let p0 = Rect::from_min_size(pos2(x0, y0), vec2(page_w, page_h));
        return Geometry {
            pages: [p0, p0],
            spread: false,
            inset: vec2(b.pad_x, inset_y),
            content: vec2(content_w, text_h),
        };
    }
    // Text width a whole number of characters: CJK lines then fill it
    // without stretching, and columns of characters line up.
    let text_w = ((page_w - 2.0 * b.pad_x) / base).floor().max(4.0) * base;
    let inset_x = (page_w - text_w) / 2.0;
    let content = vec2(text_w, (page_h - b.pad_top - b.pad_bottom).max(base * 3.0));
    let total_w = if b.spread { 2.0 * page_w } else { page_w };
    let x0 = area.center().x - total_w / 2.0;
    let y0 = (area.center().y - page_h / 2.0).max(area.top() + b.desk);
    let p0 = Rect::from_min_size(pos2(x0, y0), vec2(page_w, page_h));
    let p1 = p0.translate(vec2(page_w, 0.0));
    Geometry {
        pages: [p0, p1],
        spread: b.spread,
        inset: vec2(inset_x, b.pad_top),
        content,
    }
}

/// Abramowitz & Stegun 7.1.26 approximation of `erf(x)` (max error 1.5e-7).
fn erf_approx(x: f32) -> f32 {
    let sign = x.signum();
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_4 * t - 1.453_152) * t) + 1.421_413_7) * t - 0.284_496_74) * t
            + 0.254_829_6)
            * t
            * (-x * x).exp();
    sign * y
}

/// 1D box `[a, 1 - a]` convolved with a Gaussian of std `sigma`, smoothly
/// tapered to zero at `0` and `1` so texture edges never show a seam.
fn gaussian_box(t: f32, a: f32, sigma: f32) -> f32 {
    let s = sigma * std::f32::consts::SQRT_2;
    let raw = 0.5 * (erf_approx((t - a) / s) - erf_approx((t - (1.0 - a)) / s));
    let edge = (t.min(1.0 - t) / 0.08).clamp(0.0, 1.0);
    let smooth_edge = edge * edge * (3.0 - 2.0 * edge);
    (raw * smooth_edge).max(0.0)
}

/// Shared 64×32 white Gaussian-feathered cloud texture, uploaded once per
/// egui context and tinted at draw time for any theme.
fn spoiler_blur_texture(ctx: &egui::Context) -> egui::TextureId {
    let key = egui::Id::unique("reader-spoiler-blur-tex");
    if let Some(h) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(key)) {
        return h.id();
    }
    const W: usize = 64;
    const H: usize = 32;
    let mut vals = vec![0.0f32; W * H];
    let mut peak = 0.0f32;
    for y in 0..H {
        let v = (y as f32 + 0.5) / H as f32;
        let gy = gaussian_box(v, 0.30, 0.145);
        for x in 0..W {
            let u = (x as f32 + 0.5) / W as f32;
            let gx = gaussian_box(u, 0.24, 0.11);
            let val = gx * gy;
            vals[y * W + x] = val;
            peak = peak.max(val);
        }
    }
    let scale = if peak > 0.0 { 1.0 / peak } else { 1.0 };
    let pixels: Vec<Color32> = vals
        .into_iter()
        .map(|val| {
            let a = (val * scale * 255.0).round().clamp(0.0, 255.0) as u8;
            Color32::from_rgba_unmultiplied(255, 255, 255, a)
        })
        .collect();
    let img = egui::ColorImage {
        size: [W, H],
        source_size: vec2(W as f32, H as f32),
        pixels,
    };
    let handle = ctx.load_texture("reader:spoiler-blur", img, egui::TextureOptions::LINEAR);
    let id = handle.id();
    ctx.data_mut(|d| d.insert_temp(key, handle));
    id
}

fn paint_spoiler_blur(
    painter: &egui::Painter,
    th: &Theme,
    spoiler: &layout::Spoiler,
    off: Vec2,
    vertical: bool,
    hovered: bool,
) {
    let tex = spoiler_blur_texture(painter.ctx());
    let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
    let fp = spoiler.font_px;
    let pad = if vertical {
        vec2(fp * 0.42, fp * 0.42)
    } else {
        vec2(fp * 0.45, fp * 0.42)
    };
    let base_alpha = if th.dark { 0.42 } else { 0.38 };
    let boost = if hovered { 1.28 } else { 1.0 };
    for blob in &spoiler.blobs {
        let alpha = (base_alpha * blob.weight * boost).clamp(0.12, 0.72);
        let color = th.muted.gamma_multiply(alpha);
        painter.image(tex, blob.rect.translate(off).expand2(pad), uv, color);
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
        Item::Text {
            pos,
            galley,
            angle,
            faux,
        } => {
            if *angle == 0.0 {
                painter.galley(*pos + off, galley.clone(), th.ink);
                if let Some((g, delta)) = faux {
                    painter.galley(*pos + off + *delta, g.clone(), th.ink);
                }
            } else {
                painter.add(
                    egui::epaint::TextShape::new(*pos + off, galley.clone(), th.ink)
                        .with_angle(*angle),
                );
                if let Some((g, delta)) = faux {
                    painter.add(
                        egui::epaint::TextShape::new(*pos + off + *delta, g.clone(), th.ink)
                            .with_angle(*angle),
                    );
                }
            }
        }
        Item::Image { rect, path } => {
            let r = rect.translate(off);
            let px = [
                (r.width() * ppp).ceil() as u32,
                (r.height() * ppp).ceil() as u32,
            ];
            match images.texture(painter.ctx(), path, px) {
                images::TexState::Ready(id) => {
                    painter.image(
                        id,
                        r,
                        Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                images::TexState::Pending | images::TexState::Failed => {
                    painter.rect_filled(r, 2.0, th.bar);
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
    #[cfg(test)] items: &mut Vec<(Rect, usize, bool)>,
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
                    #[cfg(test)]
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

#[cfg(test)]
mod geometry_tests {
    use super::*;

    #[test]
    fn vertical_geometry_uses_single_landscape_page() {
        let mut s = Settings::default();
        let wide = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0));
        let horiz = geometry(&s, wide);
        assert!(horiz.spread, "horizontal uses spread on wide window");

        s.vertical = true;
        for (w, h) in [(1280.0, 800.0), (1920.0, 1080.0), (800.0, 800.0)] {
            let area = Rect::from_min_size(Pos2::ZERO, vec2(w, h));
            let g = geometry(&s, area);
            assert!(!g.spread, "vertical uses one page at {w}x{h}");
            let page = g.pages[0];
            assert!(
                page.width() > page.height(),
                "landscape at {w}x{h}: {page:?}"
            );
            assert!(
                (g.content.y % s.size).abs() < 1e-3,
                "column height snaps to whole chars: {} vs {}",
                g.content.y,
                s.size
            );
            assert!(
                area.contains_rect(page),
                "page fits inside area at {w}x{h}: {page:?}"
            );
        }
    }

    #[test]
    fn custom_page_width_and_height_horizontal() {
        let area = Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1080.0));

        // Single page: default matches the original formula (max_text + 2*pad_x, full height).
        let mut s = Settings {
            spread: false,
            ..Settings::default()
        };
        assert_eq!(s.page_width, None);
        assert_eq!(s.page_height, None);
        let def = geometry(&s, area);
        assert!(!def.spread);
        let expected_w = s.size * 34.0 + 2.0 * (s.size * 2.2 * s.margins);
        let expected_h = area.height() - 36.0;
        assert!((def.pages[0].width() - expected_w).abs() < 1e-3);
        assert!((def.pages[0].height() - expected_h).abs() < 1e-3);
        assert!((def.pages[0].top() - 18.0).abs() < 1e-3);

        // Wider and shorter than default, centered vertically and horizontally.
        s.page_width = Some(1200.0);
        s.page_height = Some(640.0);
        let custom = geometry(&s, area);
        assert!((custom.pages[0].width() - 1200.0).abs() < 1e-3);
        assert!((custom.pages[0].height() - 640.0).abs() < 1e-3);
        assert!((custom.pages[0].center().x - area.center().x).abs() < 1e-3);
        assert!((custom.pages[0].center().y - area.center().y).abs() < 1e-3);
        assert!((custom.content.x % s.size).abs() < 1e-3);
        assert!(area.contains_rect(custom.pages[0]));

        // Oversized values clamp to the window's available space.
        s.page_width = Some(5000.0);
        s.page_height = Some(5000.0);
        let clamped_hi = geometry(&s, area);
        assert!((clamped_hi.pages[0].width() - (area.width() - 36.0)).abs() < 1e-3);
        assert!((clamped_hi.pages[0].height() - expected_h).abs() < 1e-3);
        assert!(area.contains_rect(clamped_hi.pages[0]));

        // Tiny values clamp to a readable minimum.
        s.page_width = Some(10.0);
        s.page_height = Some(10.0);
        let clamped_lo = geometry(&s, area);
        let b = page_bounds(&s, area);
        assert!((clamped_lo.pages[0].width() - b.min_w).abs() < 1e-3);
        assert!((clamped_lo.pages[0].height() - b.min_h).abs() < 1e-3);
        assert!(clamped_lo.content.x >= s.size * 4.0);
        assert!(clamped_lo.content.y >= s.size * 3.0);

        // Spread mode: custom width applies to each page, clamped to half the window.
        let mut spread_s = Settings {
            spread: true,
            page_width: Some(500.0),
            page_height: Some(700.0),
            ..Settings::default()
        };
        let sp = geometry(&spread_s, area);
        assert!(sp.spread);
        assert!((sp.pages[0].width() - 500.0).abs() < 1e-3);
        assert!((sp.pages[1].width() - 500.0).abs() < 1e-3);
        assert!((sp.pages[0].height() - 700.0).abs() < 1e-3);
        assert!((sp.pages[0].right() - sp.pages[1].left()).abs() < 1e-3);
        assert!(((sp.pages[0].left() + sp.pages[1].right()) / 2.0 - area.center().x).abs() < 1e-3);

        spread_s.page_width = Some(5000.0);
        let sp_max = geometry(&spread_s, area);
        assert!((sp_max.pages[0].width() - (area.width() - 54.0) / 2.0).abs() < 1e-3);
        assert!(area.contains_rect(sp_max.pages[0]));
        assert!(area.contains_rect(sp_max.pages[1]));
    }

    #[test]
    fn custom_page_width_and_height_vertical() {
        let area = Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1080.0));
        let mut s = Settings {
            vertical: true,
            ..Settings::default()
        };
        let def = geometry(&s, area);

        // Increasing height does not unexpectedly change width.
        s.page_height = Some(920.0);
        let taller = geometry(&s, area);
        assert!((taller.pages[0].height() - 920.0).abs() < 1e-3);
        assert!((taller.pages[0].width() - def.pages[0].width()).abs() < 1e-3);
        assert!((taller.content.y % s.size).abs() < 1e-3);

        // Increasing width does not change height.
        s.page_width = Some(1600.0);
        let wider = geometry(&s, area);
        assert!((wider.pages[0].width() - 1600.0).abs() < 1e-3);
        assert!((wider.pages[0].height() - 920.0).abs() < 1e-3);
        assert!(area.contains_rect(wider.pages[0]));
    }

    #[test]
    fn page_size_settings_serde_defaults_to_none() {
        let legacy: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.page_width, None);
        assert_eq!(legacy.page_height, None);

        let custom = Settings {
            page_width: Some(820.0),
            page_height: Some(640.0),
            ..Settings::default()
        };
        let json = serde_json::to_string(&custom).unwrap();
        let roundtrip: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.page_width, Some(820.0));
        assert_eq!(roundtrip.page_height, Some(640.0));
    }

    #[test]
    fn page_dimension_row_keeps_none_until_changed() {
        let ctx = egui::Context::default();
        let mut width = None;
        let mut height = Some(1500.0);
        let mut out = ctx.run_ui(Default::default(), |ui| {
            egui::Grid::new("test-page-dims")
                .num_columns(2)
                .show(ui, |ui| {
                    page_dimension_row(ui, "Page width", &mut width, 768.0, 168.0, 1200.0, "", "");
                    // Saved value wider than current window stays untouched if user doesn't drag it.
                    page_dimension_row(ui, "Page height", &mut height, 764.0, 200.0, 764.0, "", "");
                });
        });
        out.textures_delta.clear();
        assert_eq!(width, None);
        assert_eq!(height, Some(1500.0));
    }
}

#[cfg(test)]
mod position_tests {
    use super::*;

    fn write_test_epub(path: &Path) {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let ch_body = |tag: &str| {
            let mut body = String::from(r#"<html><meta charset="utf-8"><body>"#);
            for i in 0..40 {
                body.push_str(&format!(
                    r#"<p id="p{i}">{tag}第{i}段：{}</p>"#,
                    "春江潮水连海平海上明月共潮生滟滟随波千万里何处春江无月明。".repeat(3)
                ));
            }
            body.push_str("</body></html>");
            body
        };
        let ch0 = ch_body("甲");
        let ch1 = ch_body("乙");
        let ch2 = ch_body("丙");
        let files: &[(&str, &str)] = &[
            (
                "META-INF/container.xml",
                r#"<container><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#,
            ),
            (
                "book.opf",
                r#"<package><metadata><title>Position robustness</title></metadata>
                   <manifest>
                     <item id="c0" href="c0.xhtml" media-type="application/xhtml+xml"/>
                     <item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>
                     <item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/>
                   </manifest>
                   <spine>
                     <itemref idref="c0"/>
                     <itemref idref="c1"/>
                     <itemref idref="c2"/>
                   </spine></package>"#,
            ),
            ("c0.xhtml", &ch0),
            ("c1.xhtml", &ch1),
            ("c2.xhtml", &ch2),
        ];
        for &(name, body) in files {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    fn run_frames(ctx: &egui::Context, r: &mut Reader, size: Vec2, time: &mut f64, n: usize) {
        for _ in 0..n {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                time: Some(*time),
                predicted_dt: 1.0 / 60.0,
                ..Default::default()
            };
            *time += 1.0 / 60.0;
            let mut out = ctx.run_ui(raw, |ui| {
                let _ = r.ui(ui);
            });
            out.textures_delta.clear();
        }
    }

    fn wait_ready(ctx: &egui::Context, r: &mut Reader, size: Vec2, time: &mut f64) {
        let start = std::time::Instant::now();
        while fonts::families().is_none()
            || !r.layouts.contains_key(&r.spine)
            || r.target.is_some()
            || r.progress().is_none()
        {
            run_frames(ctx, r, size, time, 1);
            assert!(start.elapsed().as_secs() < 30, "reader never became ready");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        run_frames(ctx, r, size, time, 2);
    }

    fn open_test_reader(ctx: &egui::Context, path: &Path, book_id: &str) -> Reader {
        let mut r = Reader::open(
            path,
            Some(book_id),
            false,
            ctx,
            &mut DictionarySession::default(),
        )
        .unwrap();
        r.persist = false;
        r.settings = Settings {
            spread: false,
            size: 20.0,
            ..Settings::default()
        };
        r
    }

    #[test]
    fn chapter_transition_and_font_change_never_jump_or_lose_position() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        write_test_epub(&path);

        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let size = vec2(1200.0, 800.0);
        let mut time = 1.0;

        let mut r = open_test_reader(&ctx, &path, "pos-test-1");
        r.go_chapter(0, Target::Offset(0));
        wait_ready(&ctx, &mut r, size, &mut time);
        let g = r.drawn.unwrap();

        // Walk to the last page of chapter 0 so `r.offset` is large.
        let ch0_pages = r.layouts[&0].pages.len();
        assert!(
            ch0_pages >= 4,
            "chapter 0 should span multiple pages: {ch0_pages}"
        );
        for _ in 0..ch0_pages - 1 {
            r.next(&g);
        }
        assert_eq!(r.spine, 0);
        assert_eq!(r.page, ch0_pages - 1);
        let ch0_end_offset = r.offset;
        assert!(
            ch0_end_offset > 500,
            "expected large end offset, got {ch0_end_offset}"
        );

        // Turn into chapter 1: offset and persistent_position must immediately become (1, 0).
        r.next(&g);
        assert_eq!(r.spine, 1);
        assert_eq!(r.offset, 0);
        assert_eq!(
            r.persistent_position(),
            Position {
                spine: 1,
                offset: 0
            }
        );
        wait_ready(&ctx, &mut r, size, &mut time);
        assert_eq!((r.spine, r.page, r.offset), (1, 0, 0));

        // Change sans-serif font (and body text to sans): must stay at page 0, offset 0!
        let fams = fonts::families().unwrap();
        let alt_font = fams
            .iter()
            .find(|f| {
                !f.name
                    .eq_ignore_ascii_case(r.settings.sans.as_deref().unwrap_or(""))
            })
            .map(|f| f.name.clone())
            .unwrap_or_else(|| fonts::BUNDLED.to_owned());
        r.settings.sans = Some(alt_font);
        r.settings.sans_default = true;
        time += 0.5;
        run_frames(&ctx, &mut r, size, &mut time, 4);
        wait_ready(&ctx, &mut r, size, &mut time);
        assert_eq!(
            (r.spine, r.page, r.offset),
            (1, 0, 0),
            "changing sans-serif font on page 0 of a new chapter must not jump forward"
        );
        assert_eq!(
            r.persistent_position(),
            Position {
                spine: 1,
                offset: 0
            }
        );

        // Advance deep into chapter 1, then jump back to chapter 1 start via Target::Offset(0)
        // (same path as Contents menu): offset and persistent_position must immediately reset to 0.
        let g = r.drawn.unwrap();
        for _ in 0..3 {
            r.next(&g);
        }
        assert!(r.page >= 3 && r.offset > 500);
        r.go_chapter(1, Target::Offset(0));
        assert_eq!(r.offset, 0);
        assert_eq!(
            r.persistent_position(),
            Position {
                spine: 1,
                offset: 0
            }
        );
        wait_ready(&ctx, &mut r, size, &mut time);
        assert_eq!((r.spine, r.page, r.offset), (1, 0, 0));
        assert_eq!(
            r.persistent_position(),
            Position {
                spine: 1,
                offset: 0
            }
        );
    }

    #[test]
    fn prev_across_chapters_spread_toggle_and_reflows_stay_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        write_test_epub(&path);

        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let size = vec2(1920.0, 1080.0);
        let mut time = 1.0;

        let mut r = open_test_reader(&ctx, &path, "pos-test-2");
        r.go_chapter(1, Target::Offset(0));
        wait_ready(&ctx, &mut r, size, &mut time);

        // Prev from chapter 1 page 0 -> chapter 0 Target::Last.
        // Even before layout finishes, persistent_position must not report (0, 0).
        let g = r.drawn.unwrap();
        r.prev(&g);
        assert_eq!(r.spine, 0);
        let pending_pos = r.persistent_position();
        assert_eq!(pending_pos.spine, 0);
        assert!(
            pending_pos.offset > 500,
            "prev to previous chapter must not report offset 0 before layout: {pending_pos:?}"
        );
        wait_ready(&ctx, &mut r, size, &mut time);
        let last_page = r.layouts[&0].pages.len() - 1;
        assert_eq!(r.page, last_page);
        assert_eq!(r.offset, r.layouts[&0].page_offset(last_page));

        // Move to an odd page (page 3) in one-page mode on a wide window where
        // content width is identical in 1-page and 2-page spread modes.
        r.go_chapter(1, Target::Offset(0));
        wait_ready(&ctx, &mut r, size, &mut time);
        let g = r.drawn.unwrap();
        for _ in 0..3 {
            r.next(&g);
        }
        assert_eq!(r.page, 3);
        let anchor_offset = r.offset;
        assert!(anchor_offset > 0);

        // Toggle Two pages -> spread 2..3, anchor offset preserved.
        r.settings.spread = true;
        run_frames(&ctx, &mut r, size, &mut time, 3);
        wait_ready(&ctx, &mut r, size, &mut time);
        assert_eq!(r.page, 2);
        assert_eq!(r.offset, anchor_offset);

        // Toggle back to One page -> returns to exact page 3 (not stuck on page 2!).
        r.settings.spread = false;
        run_frames(&ctx, &mut r, size, &mut time, 3);
        wait_ready(&ctx, &mut r, size, &mut time);
        assert_eq!(r.page, 3);
        assert_eq!(r.offset, anchor_offset);

        // Repeated font size & vertical/horizontal reflows must not drift `r.offset`.
        for (sz, vert) in [(26.0, false), (16.0, false), (20.0, true), (20.0, false)] {
            r.settings.size = sz;
            r.settings.vertical = vert;
            time += 0.5;
            run_frames(&ctx, &mut r, size, &mut time, 4);
            wait_ready(&ctx, &mut r, size, &mut time);
            assert_eq!(
                r.offset, anchor_offset,
                "reflow (size={sz}, vertical={vert}) must not drift offset"
            );
        }
        assert_eq!(r.page, 3);

        // Peek via search after advancing, and verify persistent_position keeps the pre-peek place.
        let pre_peek = Position {
            spine: r.spine,
            offset: r.offset,
        };
        r.search_go(search::Hit {
            spine: 2,
            range: 0..4,
            before: String::new(),
            text: String::new(),
            after: String::new(),
        });
        assert_eq!(r.persistent_position(), pre_peek);
        wait_ready(&ctx, &mut r, size, &mut time);
        assert_eq!(r.spine, 2);
        assert_eq!(r.persistent_position(), pre_peek);
        r.peek_back();
        wait_ready(&ctx, &mut r, size, &mut time);
        assert_eq!((r.spine, r.page, r.offset), (1, 3, anchor_offset));
    }
}
