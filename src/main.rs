#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use hanstat::analyze::{self, Prepared, Report};
use hanstat::anki::{self, BookCoverage, ByStatus, Difficulty, Status};
use hanstat::cache::Cache;
use hanstat::discover::{self, Candidate};
use hanstat::extract::{self, Book, EncodingChoice, SectionKind};
use hanstat::history::{self, History};
use hanstat::known;
use hanstat::lang::Language;
use hanstat::plan;
use hanstat::profile::{self, Profile};
use hanstat::script::Variant;
use hanstat::search;
use hanstat::segment::SegmenterKind;
use hanstat::settings::Settings;
use hanstat::style;
use hanstat::update::{self, Release};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

mod charts;
mod library_stats;
mod palette;
mod read_books;
mod style_scale;

use library_stats::StatsState;
use style_scale::StyleScale;

fn main() -> eframe::Result {
    // hanstat [file...] [--tab overview|characters|words|sections|notes|library|path|stats]
    // Several files are analysed one after another and the last is shown.
    let mut paths = Vec::new();
    let mut tab = None;
    // --tab library / path / stats: start in the Library view, on its books, the reading
    // path or its statistics
    // (`history`, its name before, still works).
    let mut history_tab = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--tab" {
            tab = Some(match args.next().as_deref() {
                Some("characters") => Tab::Characters,
                Some("words") => Tab::Words,
                Some("sections") => Tab::Sections,
                Some("notes") => Tab::Notes,
                Some("suggestions") => Tab::Suggestions,
                Some("library" | "history") => {
                    history_tab = Some(HistTab::Books);
                    Tab::Overview
                }
                Some("path") => {
                    history_tab = Some(HistTab::Path);
                    Tab::Overview
                }
                Some("stats") => {
                    history_tab = Some(HistTab::Stats);
                    Tab::Overview
                }
                _ => Tab::Overview,
            });
        } else {
            paths.push(PathBuf::from(a));
        }
    }
    // The window's size and place, and what UiState keeps, between runs.
    let window_file = hanstat::models::app_dir().join("window.ron");
    // Centring would override the place kept from the last run.
    let centered = !window_file.is_file();
    // History, profiles and known words are written in the background.
    hanstat::saver::defer();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("hanstat")
            .with_inner_size([1180.0, 800.0])
            .with_min_inner_size([760.0, 520.0])
            .with_drag_and_drop(true)
            // The Wayland app_id (ignored elsewhere), matching the Flatpak's
            // .desktop file so the desktop shows its name and icon.
            .with_app_id("io.github.caocaochan.hanstat")
            // Same image as the exe's icon resource (build.rs, assets/icon.ico).
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                    .expect("bundled icon is a valid PNG"),
            ),
        // egui repaints on every WM_MOVE; with vsync each of those blocks the
        // OS move loop until vblank, so dragging the window stutters. egui
        // only repaints on demand, so dropping vsync costs nothing at idle.
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            surface: eframe::egui_wgpu::SurfaceConfig {
                present_mode: eframe::wgpu::PresentMode::AutoNoVsync,
                ..eframe::egui_wgpu::SurfaceConfig::HIGH_THROUGHPUT
            },
            ..Default::default()
        },
        // Centre on the primary monitor at the first start.
        centered,
        persistence_path: Some(window_file),
        ..Default::default()
    };
    let result = eframe::run_native(
        "hanstat",
        options,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            let saved: Option<UiState> = cc.storage.and_then(|s| eframe::get_value(s, UI_KEY));
            let mut app = App {
                // Only fit the window to the Overview until it has a size of its own.
                auto_fit: saved.is_none(),
                ..App::default()
            };
            let ui = saved.unwrap_or_default();
            app.tab = tab.unwrap_or(ui.tab);
            app.history_tab = ui.history_tab;
            app.history_sort = ui.history_sort;
            app.library_hide_read = ui.library_hide_read;
            app.char_sort = ui.char_sort;
            app.word_sort = ui.word_sort;
            app.models = fetch_models(&cc.egui_ctx);
            app.updater.start(&cc.egui_ctx);
            app.anki_init(&cc.egui_ctx);
            (app.history, app.history_error) = History::load();
            (app.plan.settings, app.plan.error) = plan::Settings::load();
            if let Some(t) = history_tab {
                app.show_history = true;
                app.history_tab = t;
            }
            match paths.len() {
                0 => {}
                // As before: a history view asked for stays up.
                1 => app.start(paths.pop().unwrap(), cc.egui_ctx.clone()),
                _ => app.open_files(paths, &cc.egui_ctx),
            }
            Ok(Box::new(app))
        }),
    );
    hanstat::saver::flush();
    result
}

/// egui ships no CJK glyphs; use the system's fonts as fallbacks, in order.
/// Characters none of them has still go to eframe's system font provider.
fn install_fonts(ctx: &egui::Context) {
    let (dir, candidates) = system_fonts();
    let mut fonts = egui::FontDefinitions::default();
    // Several faces can come from one `.ttc`: read each file once.
    let mut files: HashMap<&str, Option<egui::epaint::text::Blob>> = HashMap::new();
    for &(name, file, index) in candidates {
        let blob = files.entry(file).or_insert_with(|| {
            let bytes = std::fs::read(dir.join(file)).ok()?;
            Some(Arc::new(bytes) as egui::epaint::text::Blob)
        });
        if let Some(blob) = blob {
            let data = egui::FontData::from_blob(blob.clone(), index);
            fonts.font_data.insert(name.to_owned(), Arc::new(data));
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().push(name.to_owned());
            }
        }
    }
    ctx.set_fonts(fonts);
}

/// A fallback font: its name for egui, its file in the font folder, and the
/// face to use in a `.ttc` collection.
type SystemFont = (&'static str, &'static str, u32);

/// Windows: the font folder and its CJK fonts.
#[cfg(windows)]
fn system_fonts() -> (PathBuf, &'static [SystemFont]) {
    let dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts");
    let candidates = &[
        ("yahei", "msyh.ttc", 0),             // Microsoft YaHei: BMP CJK
        ("jhenghei", "msjh.ttc", 0),          // JhengHei: extra Traditional coverage
        ("simsun-extb", "simsunb.ttf", 0),    // SimSun-ExtB: Ext B (SIP)
        ("simsun-extg", "SimsunExtG.ttf", 0), // Ext G
        ("segoe-symbol", "seguisym.ttf", 0),
    ];
    (dir, candidates)
}

/// macOS: fonts that ship with the system (from linlin56's PR #1). Only
/// TrueType-outline fonts: egui draws the CFF-outline ones (Hiragino,
/// PingFang, Apple SD Gothic Neo) with strokes missing (了 as 亅, 我 as 找),
/// and the newest PingFang's `hvgl` outlines not at all. None covers all of
/// Ext B as SimSun-ExtB does; Heiti has about 6,300 of its characters.
#[cfg(target_os = "macos")]
fn system_fonts() -> (PathBuf, &'static [SystemFont]) {
    let candidates = &[
        ("heiti-sc", "STHeiti Light.ttc", 1), // Heiti SC: BMP CJK, kana, Ext B
        // The collection's other face (Heiti TC): extra Traditional coverage,
        // as JhengHei on Windows. Characters it lacks too go to eframe's
        // system font provider, which may pick a CFF font.
        ("heiti-tc", "STHeiti Light.ttc", 0),
        ("apple-gothic", "Supplemental/AppleGothic.ttf", 0), // Hangul
        ("apple-symbols", "Apple Symbols.ttf", 0),
    ];
    (PathBuf::from("/System/Library/Fonts"), candidates)
}

/// Linux: no CJK font comes with every distribution, so the Flatpak brings
/// its own, in `share/hanstat/fonts` beside the exe's `bin` folder
/// (`HANSTAT_FONT_DIR` overrides it). Noto Sans CJK SC has the BMP's CJK
/// (Traditional too, in the Simplified forms, as YaHei on Windows), kana and
/// Hangul; its CFF outlines draw correctly, unlike Apple's. Plangothic P1 has
/// Ext B–F and P2 Ext G–H, as SimSun-ExtB and -ExtG on Windows. Without them
/// (a build run from Cargo), CJK goes to eframe's system font provider.
#[cfg(not(any(windows, target_os = "macos")))]
fn system_fonts() -> (PathBuf, &'static [SystemFont]) {
    let dir = std::env::var_os("HANSTAT_FONT_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            let exe = std::env::current_exe().ok()?;
            Some(exe.parent()?.parent()?.join("share/hanstat/fonts"))
        })
        .unwrap_or_default();
    let candidates = &[
        ("noto-sans-cjk-sc", "NotoSansCJKsc-Regular.otf", 0),
        ("plangothic-p1", "PlangothicP1-Regular.ttf", 0), // Ext B–F
        ("plangothic-p2", "PlangothicP2-Regular.ttf", 0), // Ext G–H
    ];
    (dir, candidates)
}

/// Whether files dropped on the window reach hanstat: not under Wayland on
/// Linux, where winit 0.30 has no file drag-and-drop (X11 has). winit picks
/// Wayland whenever one of these variables is set.
fn drop_works() -> bool {
    cfg!(any(windows, target_os = "macos"))
        || (std::env::var_os("WAYLAND_DISPLAY").is_none()
            && std::env::var_os("WAYLAND_SOCKET").is_none())
}

#[derive(PartialEq, Eq, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
enum Tab {
    #[default]
    Overview,
    Characters,
    Words,
    Sections,
    Suggestions,
    Notes,
}

impl Tab {
    /// The `--tab` value that reopens this tab.
    fn arg(self) -> &'static str {
        match self {
            Tab::Overview => "overview",
            Tab::Characters => "characters",
            Tab::Words => "words",
            Tab::Sections => "sections",
            Tab::Suggestions => "suggestions",
            Tab::Notes => "notes",
        }
    }
}

struct Loaded {
    book: Arc<Book>,
    prep: Arc<Prepared>,
    /// SHA-256 of the file, which identifies the book in the history.
    file_id: Option<String>,
}

enum JobMsg {
    Done(Result<Loaded, String>),
}

/// Background download of every model missing from the cache folder, started
/// at launch so a segmenter is ready the moment it's needed.
#[derive(Default)]
struct ModelFetch {
    done: AtomicUsize,
    total: AtomicUsize,
    current: Mutex<&'static str>,
    errors: Mutex<Vec<String>>,
    finished: std::sync::atomic::AtomicBool,
}

/// Download missing models (LTP first: it is the default and the smallest),
/// then load every model into memory. `None` if nothing needs downloading.
fn fetch_models(ctx: &egui::Context) -> Option<Arc<ModelFetch>> {
    use hanstat::models::{CKIP, LTP};
    let missing: Vec<&'static hanstat::models::Model> = [&LTP, &CKIP]
        .into_iter()
        .filter(|m| !m.is_cached())
        .collect();
    let state = Arc::new(ModelFetch::default());
    state.total.store(
        missing.iter().map(|m| m.total_size() as usize).sum(),
        Ordering::Relaxed,
    );
    let (s, ctx) = (state.clone(), ctx.clone());
    std::thread::spawn(move || {
        let mut base = 0usize;
        for m in &missing {
            *s.current.lock().unwrap() = m.title;
            let c = ctx.clone();
            let res = m.ensure(&|d, _| {
                s.done.store(base + d as usize, Ordering::Relaxed);
                c.request_repaint_after(std::time::Duration::from_millis(100));
            });
            if let Err(e) = res {
                s.errors.lock().unwrap().push(e);
            }
            base += m.total_size() as usize;
        }
        s.finished.store(true, Ordering::Relaxed);
        ctx.request_repaint();
        // Load into memory so the first analysis doesn't wait for it.
        if LTP.is_cached() {
            let _ = hanstat::segment::ltp_model();
        }
        if CKIP.is_cached() {
            let _ = hanstat::segment::ckip_model();
        }
    });
    (state.total.load(Ordering::Relaxed) != 0).then_some(state)
}

#[derive(Clone, Default)]
enum UpdatePhase {
    #[default]
    Idle,
    Checking,
    UpToDate,
    /// Found by a manual check with automatic updates off.
    Available(Release),
    Downloading {
        tag: String,
        done: u64,
        total: u64,
    },
    /// On disk; the running copy is still the old version until a restart.
    Installed(Release),
    Failed(String),
}

/// Self-update from GitHub releases: checked at startup and, unless turned
/// off, installed in the background; the new version runs after a restart.
struct Updater {
    settings: update::Settings,
    /// Set for dev builds, `HANSTAT_NO_UPDATE`, or an unlocatable exe.
    disabled: Option<String>,
    phase: Arc<Mutex<UpdatePhase>>,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            settings: update::Settings::load(),
            disabled: update::disabled_reason(),
            phase: Arc::default(),
        }
    }
}

impl Updater {
    fn phase(&self) -> UpdatePhase {
        self.phase.lock().unwrap().clone()
    }

    fn busy(&self) -> bool {
        matches!(
            self.phase(),
            UpdatePhase::Checking | UpdatePhase::Downloading { .. }
        )
    }

    /// At launch: delete what the previous update left, then check.
    fn start(&self, ctx: &egui::Context) {
        if self.disabled.is_some() {
            return;
        }
        std::thread::spawn(update::clean_up_here);
        if self.settings.auto_update {
            self.check(ctx, true);
        }
    }

    /// Look for a newer release; with `install`, download and install it too.
    fn check(&self, ctx: &egui::Context, install: bool) {
        if self.disabled.is_some() || self.busy() {
            return;
        }
        *self.phase.lock().unwrap() = UpdatePhase::Checking;
        let (phase, ctx) = (self.phase.clone(), ctx.clone());
        std::thread::spawn(move || {
            let next = match update::check() {
                Ok(None) => UpdatePhase::UpToDate,
                Ok(Some(rel)) if install => return Self::install_now(&phase, &ctx, rel),
                Ok(Some(rel)) => UpdatePhase::Available(rel),
                Err(e) => UpdatePhase::Failed(format!("Checking for updates failed: {e}")),
            };
            *phase.lock().unwrap() = next;
            ctx.request_repaint();
        });
    }

    fn install(&self, ctx: &egui::Context, rel: Release) {
        if self.disabled.is_some() || self.busy() {
            return;
        }
        let (phase, ctx) = (self.phase.clone(), ctx.clone());
        std::thread::spawn(move || Self::install_now(&phase, &ctx, rel));
    }

    fn install_now(phase: &Mutex<UpdatePhase>, ctx: &egui::Context, rel: Release) {
        let set = |p: UpdatePhase| {
            *phase.lock().unwrap() = p;
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        };
        set(UpdatePhase::Downloading {
            tag: rel.tag.clone(),
            done: 0,
            total: rel.size,
        });
        let result = update::download(&rel, &|done, total| {
            set(UpdatePhase::Downloading {
                tag: rel.tag.clone(),
                done,
                total,
            })
        })
        .and_then(|zip| update::install_here(&zip));
        set(match result {
            Ok(()) => UpdatePhase::Installed(rel),
            Err(e) => UpdatePhase::Failed(format!("Updating to {} failed: {e}", rel.tag)),
        });
        ctx.request_repaint();
    }
}

/// Release notes for a tooltip, cut short if long.
fn notes_preview(rel: &Release) -> String {
    let mut s: String = rel.notes.chars().take(1500).collect();
    if s.len() < rel.notes.len() {
        s.push_str("\n…");
    }
    s
}

struct Job {
    rx: Receiver<JobMsg>,
    done: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    stage: Arc<Mutex<String>>,
    /// Set when another analysis replaces this one: it stops soon after.
    cancel: Arc<AtomicBool>,
}

/// What is kept separately for each language: the user dictionary, the Anki
/// selection and the word lists / marked words. The current language's data
/// lives in `App`'s own fields; the other one waits here.
#[derive(Default)]
struct LangData {
    user_words: Vec<String>,
    anki: AnkiState,
    known: KnownState,
}

struct App {
    path: Option<PathBuf>,
    /// The language of the open book (detected), or chosen with the badge
    /// while no book is open. Picks which `LangData` is in use.
    language: Language,
    /// Data of the languages not in use.
    stash: HashMap<Language, LangData>,
    /// The Chinese segmenter choice (a Korean book always uses Kiwi, a Japanese one Sudachi).
    segmenter: SegmenterKind,
    encoding: EncodingChoice,
    /// Forced words of the current language, saved in `dictionary.txt`
    /// (see [`hanstat::user_dict_path`]).
    user_words: Vec<String>,
    /// The "Force a word" text box, the Forced words window, and a load/save error.
    force_input: String,
    forced_window: bool,
    dict_error: Option<String>,

    book: Option<Arc<Book>>,
    prep: Option<Arc<Prepared>>,
    included: Vec<bool>,
    report: Option<Report>,
    char_cum: Vec<f64>,
    word_cum: Vec<f64>,

    job: Option<Job>,
    /// Errors shown above the content until dismissed, each under a key
    /// saying where it came from, so a new error replaces only its own kind.
    errors: Vec<(&'static str, String)>,
    tab: Tab,
    char_filter: String,
    word_filter: String,
    /// The word selected in the Words tab (its key), moved with the arrow keys.
    word_sel: Option<String>,
    /// Its row in the filtered list, to select the row now there if it drops
    /// out of the list after A (the "Not known" filter).
    word_sel_pos: usize,
    word_sel_refill: bool,
    /// The open book's reading speed, where it was changed from the default
    /// in the Settings window (`Settings::cpm` and so on) to try another; reset when
    /// another book is opened. Chinese: characters per minute.
    cpm: Option<f32>,
    /// Korean: words per minute (as counted: content words).
    wpm: Option<f32>,
    /// Japanese: kanji and kana per minute.
    ja_cpm: Option<f32>,
    /// `settings` changed by a drag still in progress: saved when it ends.
    settings_unsaved: bool,
    toast: Option<(String, f64)>,

    suggestions: Option<Vec<Candidate>>,
    suggest_rx: Option<Receiver<Vec<Candidate>>>,
    /// Word discovery waits for this time (egui seconds), so that ticking
    /// section after section starts it once; and the running one's stop flag.
    discover_due: Option<std::time::Instant>,
    discover_cancel: Option<Arc<AtomicBool>>,
    selected: HashSet<String>,

    anki: AnkiState,
    anki_filter: AnkiFilter,
    known: KnownState,
    /// Grow the window to fit the Overview next time it is drawn.
    fit_pending: bool,
    /// Startup download of missing models.
    models: Option<Arc<ModelFetch>>,
    /// Keep fitting until this time: late content (the suggestions banner,
    /// the Anki section) can still push the Overview down after the first frame.
    fit_until: f64,
    /// Inner height last asked for, so a resize still in flight isn't requested again.
    fit_requested: f32,
    updater: Updater,
    /// Saved analysis results; `None` when turned off (`HANSTAT_NO_CACHE`).
    cache: Option<Cache>,
    /// Entries and bytes in `cache`, for the Notes tab (read when a book is analysed).
    cache_usage: (usize, u64),

    /// Books analysed before (history.json).
    history: History,
    history_error: Option<String>,
    /// The History view replaces the book's tabs.
    show_history: bool,
    history_sort: (HistCol, bool),
    /// The Library's search box (Books and Reading path; [`search`]).
    library_filter: String,
    /// SHA-256 of the open book's file, and of the book `report` was built
    /// from (they differ between a new book arriving and its report).
    book_id: Option<String>,
    report_id: Option<String>,
    /// Cover thumbnails by book id, loaded when first shown (`None`: the book has none).
    covers: HashMap<String, Option<egui::TextureHandle>>,
    /// The next recording is a fresh analysis: save it even if nothing changed.
    history_touch: bool,
    /// The History view shows the books, the reading path or the library's statistics.
    history_tab: HistTab,
    plan: PlanState,
    stats: StatsState,
    /// What the open book's profile was last written from (id, hash), so
    /// that it is written again only when its words or names change.
    profile_written: Option<(String, u64)>,
    /// Bumped whenever a profile is written or removed: the reading path reloads.
    profiles_version: u64,
    batch: Option<Batch>,
    /// The Settings window's settings.
    settings: Settings,
    /// Writing-style ratings ([`style`]).
    style: StyleState,
    /// Keeping the Library's rows up to date in the background.
    auto: AutoRefresh,
    /// The Mark read window, while open.
    read_dialog: Option<read_books::ReadDialog>,
    /// Known words of the Library's languages that are neither in use nor
    /// set aside, read from their files for their books' read marks
    /// ([`App::load_read_marks`]).
    known_disk: HashMap<Language, known::Settings>,
    /// The Library's Books tab leaves out the books marked read.
    library_hide_read: bool,
    /// Which Library files and word data exist, checked in the background.
    exists: Exists,
    /// A book just removed from the Library, while it can be put back.
    removed: Option<Removed>,
    /// The Settings window (⚙ in the status bar).
    settings_window: bool,
    /// Sort column and direction (descending) of the Characters and Words tables.
    char_sort: (CharCol, bool),
    word_sort: (WordCol, bool),
    /// The rows of those tables (filtered and sorted) and what they were made from.
    char_rows: Option<(RowsKey<CharCol>, Arc<Vec<usize>>)>,
    word_rows: Option<(RowsKey<WordCol>, Arc<Vec<usize>>)>,
    /// Bumped whenever the report or its known-word statuses change.
    report_gen: u64,
    /// Grow the window to fit the Overview: only until the window has a size
    /// of its own (saved from an earlier run, or resized by hand).
    auto_fit: bool,
    /// The window's inner size last frame, to notice a resize by hand.
    last_inner: Option<egui::Vec2>,
    /// [`App::lang_inputs`] by language, with a quick hash of what they were
    /// made from and the known words they were made with.
    inputs_memo: HashMap<Language, (u64, Option<Arc<anki::Knowledge>>, Inputs)>,
}

/// What a Library row in a language is computed from besides its book, its
/// sections and its segmenter ([`App::lang_inputs`]), as two digests:
/// `analysis` changes the counts (the build, the user dictionary, the known
/// words given to the segmenter, character folding); `coverage` changes only
/// what is known of them (the known words, the hapax setting). A row whose
/// analysis digest is current is updated from its saved word data alone.
#[derive(Clone, PartialEq, Debug)]
struct Inputs {
    analysis: String,
    coverage: String,
}

impl Inputs {
    /// As kept in a Library entry.
    fn joined(&self) -> String {
        format!("{}/{}", self.analysis, self.coverage)
    }

    /// The analysis part of an entry's inputs (`None` for one kept before
    /// they were split).
    fn analysis_of(entry: &str) -> Option<&str> {
        entry.split_once('/').map(|(a, _)| a)
    }
}

/// A book taken out of the Library with ✖: its place and entry, until when
/// it can be put back, after which its cover and word data are deleted.
struct Removed {
    index: usize,
    entry: history::Entry,
    until: f64,
}

/// How long a removed book can be put back (seconds).
const UNDO_SECS: f64 = 10.0;

/// Columns of the Characters table that sort.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
enum CharCol {
    Rank,
    Char,
    Status,
}

/// Columns of the Words table that sort.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
enum WordCol {
    Rank,
    Word,
    Status,
    Length,
}

/// A table header: its title, the column it sorts by with whether that
/// starts largest first (`None`: it doesn't sort), and its hover text.
type SortHead<C> = (&'static str, Option<(C, bool)>, &'static str);

/// What the rows of the Characters or Words table were made from.
#[derive(Clone, PartialEq)]
struct RowsKey<C> {
    generation: u64,
    filter: String,
    known: AnkiFilter,
    sort: (C, bool),
}

/// What is kept between runs (besides the window, which eframe keeps).
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct UiState {
    tab: Tab,
    history_tab: HistTab,
    history_sort: (HistCol, bool),
    library_hide_read: bool,
    char_sort: (CharCol, bool),
    word_sort: (WordCol, bool),
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            tab: Tab::Overview,
            history_tab: HistTab::Books,
            history_sort: (HistCol::Analysed, true),
            library_hide_read: false,
            char_sort: (CharCol::Rank, false),
            word_sort: (WordCol::Rank, false),
        }
    }
}

/// eframe's key for [`UiState`].
const UI_KEY: &str = "hanstat-ui";

/// Whether the Library's book files and word data exist, as last checked.
/// Checking takes a moment per file, longer on a sleeping drive or a network
/// share, so it is done in the background every few seconds instead of
/// while drawing.
#[derive(Default)]
struct Exists {
    known: Arc<Mutex<HashMap<PathBuf, bool>>>,
    busy: Arc<AtomicBool>,
    next: f64,
}

/// Seconds between checks of [`Exists`].
const EXISTS_EVERY: f64 = 5.0;

impl Exists {
    /// As last checked; a file not checked yet counts as there.
    fn get(&self, p: &Path) -> bool {
        self.known.lock().unwrap().get(p).copied().unwrap_or(true)
    }

    /// What was just written or deleted.
    fn set(&self, p: PathBuf, there: bool) {
        self.known.lock().unwrap().insert(p, there);
    }

    /// Check `paths` again in the background, at most every few seconds.
    fn refresh(&mut self, paths: impl FnOnce() -> Vec<PathBuf>, now: f64, ctx: &egui::Context) {
        if now < self.next || self.busy.swap(true, Ordering::Relaxed) {
            return;
        }
        self.next = now + EXISTS_EVERY;
        let paths = paths();
        let (known, busy, c) = (self.known.clone(), self.busy.clone(), ctx.clone());
        std::thread::spawn(move || {
            let found: Vec<(PathBuf, bool)> = paths
                .into_iter()
                .map(|p| {
                    let there = p.is_file();
                    (p, there)
                })
                .collect();
            let changed = {
                let mut k = known.lock().unwrap();
                let mut changed = false;
                for (p, t) in found {
                    changed |= k.insert(p, t) != Some(t);
                }
                changed
            };
            busy.store(false, Ordering::Relaxed);
            if changed {
                c.request_repaint();
            }
        });
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(EXISTS_EVERY));
    }
}

/// Library rows are analysed again in the background when what they were
/// computed from changes ([`App::auto_refresh`]).
#[derive(Default)]
struct AutoRefresh {
    /// Next time to look (seconds of `egui` time).
    next_scan: f64,
    /// Each language's inputs ([`App::lang_inputs`]) as last seen, and when they last changed.
    seen: HashMap<Language, Inputs>,
    changed_at: f64,
    /// Stopped by the user with these inputs: not started again until they change.
    paused: Option<HashMap<Language, Inputs>>,
    /// Books that could not be analysed (or are missing), by path, with the
    /// inputs tried: not tried again until those change.
    failed: HashMap<PathBuf, Inputs>,
    /// Digests of the known words, by the `Arc` they were computed for.
    knowledge: Vec<(Arc<anki::Knowledge>, String)>,
}

/// Quiet time after the last change before the background update starts,
/// so a burst of changes (marking word after word known) costs one run.
const AUTO_REFRESH_DELAY: f64 = 5.0;

#[derive(Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
enum HistTab {
    #[default]
    Books,
    Path,
    Stats,
}

/// The reading path ([`plan`]) of the History's books in the current language.
#[derive(Default)]
struct PlanState {
    settings: plan::Settings,
    error: Option<String>,
    /// History ids of the books with a profile, and what they were listed for.
    have: Option<((Language, u64, usize), Vec<String>)>,
    /// The library last built, and what from.
    lib: Option<(LibKey, Arc<plan::Library>)>,
    /// What `result`, or the running job, was computed from.
    inputs: Option<PlanInputs>,
    result: Option<PlanDone>,
    job: Option<(Receiver<PlanDone>, Arc<AtomicBool>)>,
}

/// Writing-style ratings: those made, and the rater while the setting is on.
#[derive(Default)]
struct StyleState {
    store: style::Store,
    rater: Option<style::Rater>,
    /// Books queued this session (rated, skipped or waiting): each is asked once.
    asked: HashSet<String>,
    /// Why a book was not rated, by History id.
    skipped: HashMap<String, String>,
    /// Bumped when a rating arrives.
    version: u64,
    error: Option<String>,
}

impl StyleState {
    fn load() -> Self {
        let (store, error) = style::Store::load();
        StyleState {
            store,
            error,
            ..Default::default()
        }
    }

    /// Stop rating (the server too) and forget what was asked this session.
    fn stop(&mut self) {
        self.rater = None;
        self.asked.clear();
        self.skipped.clear();
    }
}

#[derive(Clone, PartialEq)]
struct LibKey {
    lang: Language,
    fold: bool,
    /// [`Settings::skip_hapax`].
    skip_hapax: bool,
    ids: Vec<String>,
    version: u64,
}

#[derive(Clone, PartialEq)]
struct PlanInputs {
    lib: LibKey,
    /// The known words (`Arc` address; 0: none).
    knowledge: usize,
    params: plan::Params,
    /// The books on the path, by History id.
    books: Vec<String>,
    manual: Option<Vec<String>>,
    /// Target offsets from writing style, by History id (none: 0).
    offsets: Vec<(String, f64)>,
}

struct PlanDone {
    key: LibKey,
    lib: Arc<plan::Library>,
    path: plan::Path,
    /// Reading the book with the most words known first, then the next.
    naive: plan::Path,
    /// With an order set by hand: the planner's own.
    optimised: Option<plan::Path>,
    known_words: usize,
    secs: f64,
}

/// Books analysed one after another in the background, each recorded in the
/// History: several opened at once, books with no word data yet, or the
/// Library's books again.
struct Batch {
    mode: BatchMode,
    /// Each language's inputs ([`App::lang_inputs`]) the books are analysed with.
    inputs: HashMap<Language, Inputs>,
    rx: Receiver<BatchMsg>,
    done: Arc<AtomicUsize>,
    /// Including `then_open`.
    total: usize,
    current: Arc<Mutex<String>>,
    cancel: Arc<AtomicBool>,
    failed: Vec<String>,
    /// Opened (analysed and shown) once the others are done, unless stopped.
    then_open: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BatchMode {
    /// Files opened together: added to the Library as new.
    Open,
    /// Library books, asked for: their rows are replaced where they are.
    Refresh,
    /// Library books whose inputs changed ([`App::auto_refresh`]): the same,
    /// and given way to anything else.
    Auto,
}

/// One book for a batch: its label, file, Chinese segmenter, and the sections
/// to include (`None`: the book's defaults).
struct BatchItem {
    label: String,
    path: PathBuf,
    kind: SegmenterKind,
    sections: Option<Vec<bool>>,
    /// A Library row of which only the known words changed: updated from its
    /// saved word data ([`coverage_only`]), or analysed again if that can't be.
    refresh: Option<Box<history::Entry>>,
}

enum BatchMsg {
    /// With whether its word data changed.
    Book(Box<history::Entry>, bool),
    /// Label, file, error.
    Failed(String, PathBuf, String),
    Finished,
}

/// What a click in the reading path asks for.
enum PlanAction {
    Open(PathBuf),
    Move(usize, isize),
    LeaveOut(String),
    Include(String),
    MarkRead(String),
    SaveList(usize),
    Optimise,
    Backfill,
}

/// Columns of the History table that sort.
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum HistCol {
    Title,
    Language,
    Unique,
    Known,
    Chars,
    Words,
    Time,
    Analysed,
    Style,
    Read,
    Difficulty,
}

enum AnkiMsg {
    Opened(Result<anki::Collection, String>),
    Built(Box<Result<anki::Knowledge, String>>),
}

#[derive(Default)]
struct AnkiState {
    settings: anki::Settings,
    profiles: Vec<anki::Profile>,
    col: Option<Arc<Mutex<anki::Collection>>>,
    info: Option<Arc<anki::CollectionInfo>>,
    knowledge: Option<Arc<anki::Knowledge>>,
    /// One background job at a time (open or classify).
    rx: Option<Receiver<AnkiMsg>>,
    busy: &'static str,
    /// The selection changed while a job was running: classify again afterwards.
    dirty: bool,
    error: Option<String>,
    window: bool,
    show_unused_types: bool,
    /// Segmentation used for the current (or running) classification.
    built_with: Option<anki::SegConfig>,
}

/// Vocabulary levels and word lists marked known (saved in known.json, known-ko.json, known-ja.json).
#[derive(Default)]
struct KnownState {
    settings: known::Settings,
    error: Option<String>,
    window: bool,
    /// Anki (if on) plus these lists: what the book is compared with.
    merged: Option<Arc<anki::Knowledge>>,
    /// What `merged` was made from, so it is made again only when that
    /// changes: everything keyed by it (the Library's rows, the reading path,
    /// the statistics) is then left alone.
    merged_from: Option<MergedFrom>,
}

/// What [`KnownState::merged`] is made from (see [`read_books::merged_knowledge`]).
#[derive(Clone)]
struct MergedFrom {
    /// The Anki classification in use (none while Anki is off).
    anki: Option<Arc<anki::Knowledge>>,
    match_across_scripts: bool,
    known: known::Settings,
}

impl PartialEq for MergedFrom {
    fn eq(&self, o: &Self) -> bool {
        let same_anki = match (&self.anki, &o.anki) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        same_anki && self.match_across_scripts == o.match_across_scripts && self.known == o.known
    }
}

/// Word discovery waits this long after the report last changed.
const DISCOVER_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum AnkiFilter {
    #[default]
    All,
    NotKnown,
    Known,
    Is(Status),
}

impl AnkiFilter {
    const OPTIONS: [AnkiFilter; 9] = [
        AnkiFilter::All,
        AnkiFilter::NotKnown,
        AnkiFilter::Known,
        AnkiFilter::Is(Status::Mature),
        AnkiFilter::Is(Status::Young),
        AnkiFilter::Is(Status::Listed),
        AnkiFilter::Is(Status::Learning),
        AnkiFilter::Is(Status::New),
        AnkiFilter::Is(Status::NotInAnki),
    ];
    fn label(self) -> &'static str {
        match self {
            AnkiFilter::All => "All",
            AnkiFilter::NotKnown => "Not known",
            AnkiFilter::Known => "Known",
            AnkiFilter::Is(s) => s.label(),
        }
    }
    fn matches(self, s: Status) -> bool {
        match self {
            AnkiFilter::All => true,
            AnkiFilter::NotKnown => !s.is_known(),
            AnkiFilter::Known => s.is_known(),
            AnkiFilter::Is(x) => s == x,
        }
    }
}

/// Text and colour of the mode badge at the right of the toolbar.
fn mode_badge(lang: Language, variant: Option<Variant>) -> (&'static str, Color32) {
    match (lang, variant) {
        (Language::Korean, _) => ("한국어", Color32::from_rgb(0, 86, 168)),
        (Language::Japanese, _) => ("日本語", Color32::from_rgb(120, 60, 150)),
        (Language::Chinese, Some(Variant::Simplified)) => {
            ("简体中文", Color32::from_rgb(196, 43, 43))
        }
        (Language::Chinese, Some(Variant::Traditional)) => {
            ("繁體中文", Color32::from_rgb(186, 110, 16))
        }
        (Language::Chinese, Some(Variant::Mixed)) => ("简繁混合", Color32::from_rgb(160, 70, 40)),
        (Language::Chinese, _) => ("中文", Color32::from_rgb(150, 60, 60)),
    }
}

fn language_label(lang: Language) -> &'static str {
    match lang {
        Language::Chinese => "中文 (Chinese)",
        Language::Korean => "한국어 (Korean)",
        Language::Japanese => "日本語 (Japanese)",
    }
}

fn status_colour(s: Status) -> Color32 {
    let (dark, light) = match s {
        Status::Mature => ((80, 170, 100), (28, 125, 55)),
        Status::Young => ((140, 200, 120), (70, 125, 30)),
        Status::Listed => ((90, 180, 190), (0, 118, 135)),
        Status::Learning => ((220, 170, 60), (160, 95, 0)),
        Status::New => ((100, 150, 220), (35, 95, 190)),
        Status::NotInAnki => ((160, 160, 160), (110, 110, 110)),
    };
    palette::pick(
        Color32::from_rgb(dark.0, dark.1, dark.2),
        Color32::from_rgb(light.0, light.1, light.2),
    )
}

fn status_text(s: Status) -> RichText {
    let t = RichText::new(s.label());
    if s == Status::NotInAnki {
        t.weak()
    } else {
        t.color(status_colour(s))
    }
}

impl Default for App {
    fn default() -> Self {
        Self {
            path: None,
            language: Language::Chinese,
            stash: HashMap::new(),
            segmenter: SegmenterKind::Auto,
            encoding: EncodingChoice::Auto,
            user_words: Vec::new(),
            force_input: String::new(),
            forced_window: false,
            dict_error: None,
            book: None,
            prep: None,
            included: Vec::new(),
            report: None,
            char_cum: Vec::new(),
            word_cum: Vec::new(),
            job: None,
            errors: Vec::new(),
            tab: Tab::Overview,
            char_filter: String::new(),
            word_filter: String::new(),
            word_sel: None,
            word_sel_pos: 0,
            word_sel_refill: false,
            cpm: None,
            wpm: None,
            ja_cpm: None,
            settings_unsaved: false,
            toast: None,
            suggestions: None,
            suggest_rx: None,
            discover_due: None,
            discover_cancel: None,
            selected: HashSet::new(),
            anki: AnkiState::default(),
            anki_filter: AnkiFilter::All,
            known: KnownState::default(),
            fit_pending: false,
            models: None,
            fit_until: 0.0,
            fit_requested: 0.0,
            updater: Updater::default(),
            cache: Cache::open(),
            cache_usage: (0, 0),
            history: History::default(),
            history_error: None,
            show_history: false,
            history_sort: (HistCol::Analysed, true),
            library_filter: String::new(),
            book_id: None,
            report_id: None,
            history_touch: false,
            covers: HashMap::new(),
            history_tab: HistTab::Books,
            plan: PlanState::default(),
            stats: StatsState::load(),
            profile_written: None,
            profiles_version: 0,
            batch: None,
            settings: Settings::load(),
            style: StyleState::load(),
            auto: AutoRefresh::default(),
            read_dialog: None,
            known_disk: HashMap::new(),
            library_hide_read: false,
            exists: Exists::default(),
            removed: None,
            settings_window: false,
            char_sort: (CharCol::Rank, false),
            word_sort: (WordCol::Rank, false),
            char_rows: None,
            word_rows: None,
            report_gen: 0,
            auto_fit: true,
            last_inner: None,
            inputs_memo: HashMap::new(),
        }
    }
}

fn cumulative(counts: impl Iterator<Item = u32>, total: u64) -> Vec<f64> {
    let mut acc = 0u64;
    counts
        .map(|n| {
            acc += n as u64;
            acc as f64 * 100.0 / total.max(1) as f64
        })
        .collect()
}

impl App {
    fn start(&mut self, path: PathBuf, ctx: egui::Context) {
        // An analysis still running (another book, or other options) stops.
        if let Some(j) = self.job.take() {
            j.cancel.store(true, Ordering::Relaxed);
        }
        self.path = Some(path.clone());
        self.clear_error("open");
        let (tx, rx) = channel();
        let done = Arc::new(AtomicUsize::new(0));
        let total = Arc::new(AtomicUsize::new(0));
        let stage = Arc::new(Mutex::new("Reading file…".to_owned()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (d2, t2, s2, c3) = (done.clone(), total.clone(), stage.clone(), cancel.clone());
        let (kind, enc) = (self.segmenter, self.encoding);
        let cache = self.cache.clone();
        // The book's language is only known once it is read: take each language's dictionary.
        let mut words_by_lang: HashMap<Language, Vec<String>> = self
            .stash
            .iter()
            .map(|(l, d)| (*l, d.user_words.clone()))
            .collect();
        words_by_lang.insert(self.language, self.user_words.clone());
        // Imported word lists join words the segmenter split; a language
        // whose settings aren't loaded yet is read from disk.
        let mut known_by_lang: HashMap<Language, Vec<String>> = self
            .stash
            .iter()
            .map(|(l, d)| (*l, d.known.settings.segmenter_words()))
            .collect();
        known_by_lang.insert(self.language, self.known.settings.segmenter_words());
        // Reuse the already-extracted book if only segmentation options changed.
        let cached = self
            .book
            .clone()
            .filter(|_| self.path.as_deref() == Some(path.as_path()));
        std::thread::spawn(move || {
            let res = (|| {
                let book = match cached {
                    Some(b) => b,
                    None => Arc::new(extract::load(&path, enc)?),
                };
                // First use of a model: download it (pinned + checksummed) with visible progress.
                let lang = analyze::detect_language(&book);
                let words = words_by_lang
                    .remove(&lang)
                    .unwrap_or_else(|| hanstat::load_user_dict(lang).0);
                let known_words = known_by_lang
                    .remove(&lang)
                    .unwrap_or_else(|| known::Settings::load_for(lang).0.segmenter_words());
                let (tr, si) = analyze::detect_variant(&book);
                let resolved = kind.resolve_in(lang, hanstat::script::classify(tr, si));
                if let Some(m) = hanstat::segment::model_for(resolved)
                    && !m.is_cached()
                {
                    *s2.lock().unwrap() = format!(
                        "Downloading {} ({:.0} MB, first use only)…",
                        m.title,
                        m.total_size() as f64 / 1e6
                    );
                    let c2 = ctx.clone();
                    m.ensure(&|d, t| {
                        t2.store(t as usize, Ordering::Relaxed);
                        d2.store(d as usize, Ordering::Relaxed);
                        c2.request_repaint_after(std::time::Duration::from_millis(100));
                    })?;
                    d2.store(0, Ordering::Relaxed);
                }
                *s2.lock().unwrap() = format!("Segmenting with {}…", resolved.label());
                t2.store(
                    book.sections.iter().map(|s| s.paragraphs.len()).sum(),
                    Ordering::Relaxed,
                );
                let c2 = ctx.clone();
                let prep = analyze::analyze_book_cancellable(
                    &book,
                    kind,
                    &words,
                    &known_words,
                    cache.as_ref(),
                    &|d, _| {
                        d2.fetch_max(d, Ordering::Relaxed);
                        c2.request_repaint_after(std::time::Duration::from_millis(100));
                    },
                    &c3,
                )?;
                let file_id = history::file_id(&path).ok();
                // The cover's thumbnail for the History view; the same file always has the same cover.
                if let (Some(id), Some(cover)) = (&file_id, &book.cover) {
                    let dest = history::cover_path(id);
                    if !dest.is_file() {
                        let _ = history::save_cover(cover, &dest);
                    }
                }
                Ok(Loaded {
                    book,
                    prep: Arc::new(prep),
                    file_id,
                })
            })();
            let _ = tx.send(JobMsg::Done(res));
            ctx.request_repaint();
        });
        self.job = Some(Job {
            rx,
            done,
            total,
            stage,
            cancel,
        });
    }

    /// Show `e` above the content, replacing an earlier error of the same kind.
    fn set_error(&mut self, kind: &'static str, e: String) {
        self.errors.retain(|(k, _)| *k != kind);
        self.errors.push((kind, e));
    }

    fn clear_error(&mut self, kind: &'static str) {
        self.errors.retain(|(k, _)| *k != kind);
    }

    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.show_history = false;
        self.book = None;
        self.prep = None;
        self.report = None;
        self.encoding = EncodingChoice::Auto;
        self.start(path, ctx.clone());
    }

    /// Re-run with the same file (e.g. different segmenter/dictionary/encoding).
    fn rerun(&mut self, ctx: &egui::Context, reextract: bool) {
        if let Some(p) = self.path.clone() {
            if reextract {
                self.book = None;
            }
            self.start(p, ctx.clone());
        }
    }

    fn rebuild_report(&mut self) {
        if let (Some(b), Some(p)) = (&self.book, &self.prep) {
            let r = analyze::build_report(b, p, &self.included, self.fold_chars());
            self.char_cum = cumulative(r.char_freq.iter().map(|x| x.1), r.han_total);
            self.word_cum = cumulative(r.word_freq.iter().map(|x| x.1), r.words_total);
            // Word discovery depends on the included sections and the
            // segmenter's output: the one running is out of date, and the
            // next starts after a pause (`start_discovery`).
            if let Some(c) = self.discover_cancel.take() {
                c.store(true, Ordering::Relaxed);
            }
            self.suggest_rx = None;
            self.discover_due = Some(std::time::Instant::now() + DISCOVER_DELAY);
            self.report = Some(r);
            self.report_id = self.book_id.clone();
            self.report_gen += 1;
            self.refresh_anki_coverage();
        }
    }

    /// Start word discovery once [`DISCOVER_DELAY`] has passed since the
    /// report last changed.
    fn start_discovery(&mut self, ctx: &egui::Context) {
        let Some(due) = self.discover_due else { return };
        let now = std::time::Instant::now();
        if now < due {
            ctx.request_repaint_after(due - now);
            return;
        }
        self.discover_due = None;
        let (Some(book), Some(r)) = (&self.book, &self.report) else {
            return;
        };
        let (tx, rx) = channel();
        let (book, included, word_freq) =
            (book.clone(), self.included.clone(), r.word_freq.clone());
        let known: HashSet<String> = self.user_words.iter().cloned().collect();
        let opts = discover::Options::for_size(r.han_total);
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(found) =
                discover::discover_cancellable(&book, &included, &word_freq, &known, &opts, &c2)
            {
                let _ = tx.send(found);
                ctx.request_repaint();
            }
        });
        self.discover_cancel = Some(cancel);
        self.suggest_rx = Some(rx);
    }

    // ----- Anki -----

    /// Load the current language's saved settings: forced words, Anki, known words.
    fn anki_init(&mut self, ctx: &egui::Context) {
        let (w, warn) = hanstat::load_user_dict(self.language);
        self.user_words = w;
        self.dict_error = warn;
        let (s, warn) = anki::Settings::load_for(self.language);
        self.anki.settings = s;
        self.anki.error = warn;
        // Read from its file (again): any read marks changed there are in it.
        self.known_disk.remove(&self.language);
        let (k, warn) = known::Settings::load_for(self.language);
        self.known.settings = k;
        self.known.error = warn;
        self.refresh_anki_coverage();
        self.anki.profiles = anki::find_profiles();
        if self.anki.settings.enabled && self.anki.settings.collection.is_some() {
            self.anki_open(ctx);
        }
    }

    /// Switch to `lang`'s user dictionary, Anki selection and known words,
    /// loading them from disk the first time. Open windows stay open and
    /// show the new language's settings.
    fn set_language(&mut self, lang: Language, ctx: &egui::Context) {
        if lang == self.language {
            return;
        }
        let (anki_window, known_window) = (self.anki.window, self.known.window);
        let current = LangData {
            user_words: std::mem::take(&mut self.user_words),
            anki: std::mem::take(&mut self.anki),
            known: std::mem::take(&mut self.known),
        };
        self.stash.insert(self.language, current);
        self.language = lang;
        match self.stash.remove(&lang) {
            Some(d) => {
                self.user_words = d.user_words;
                self.anki = d.anki;
                self.known = d.known;
                self.refresh_anki_coverage();
            }
            None => self.anki_init(ctx),
        }
        self.anki.window = anki_window;
        self.known.window = known_window;
        if anki_window && self.anki.col.is_none() && self.anki.rx.is_none() {
            self.anki_open_default(ctx);
        }
    }

    /// Open the saved collection, or the first profile's if none is chosen yet.
    fn anki_open_default(&mut self, ctx: &egui::Context) {
        if self.anki.settings.collection.is_none() {
            self.anki.profiles = anki::find_profiles();
            self.anki.settings.collection =
                self.anki.profiles.first().map(|p| p.collection.clone());
        }
        self.anki_open(ctx);
    }

    fn anki_open(&mut self, ctx: &egui::Context) {
        let Some(path) = self.anki.settings.collection.clone() else {
            return;
        };
        self.anki.error = None;
        self.anki.col = None;
        self.anki.info = None;
        self.anki.knowledge = None;
        self.anki.dirty = false;
        self.anki.busy = "Reading Anki collection…";
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(AnkiMsg::Opened(anki::Collection::open(&path)));
            ctx.request_repaint();
        });
        // Replacing the receiver discards any job still running for the old collection.
        self.anki.rx = Some(rx);
        self.refresh_anki_coverage();
    }

    /// Sentence fields are segmented like the current book.
    fn anki_seg_config(&self) -> anki::SegConfig {
        anki::SegConfig::for_language(
            self.language,
            self.segmenter,
            self.prep
                .as_ref()
                .filter(|p| p.language == self.language)
                .map(|p| p.segmenter_kind),
            &self.user_words,
        )
    }

    fn anki_rebuild(&mut self, ctx: &egui::Context) {
        if self.anki.rx.is_some() {
            self.anki.dirty = true;
            return;
        }
        let Some(col) = self.anki.col.clone() else {
            return;
        };
        if !self.anki.settings.has_selection() {
            self.anki.knowledge = None;
            self.refresh_anki_coverage();
            return;
        }
        self.anki.busy = if self.language == Language::Korean && !hanstat::models::KIWI.is_cached()
        {
            "Downloading Kiwi (about 130 MB, first Korean use only), then classifying Anki notes…"
        } else if self.language == Language::Japanese
            && !hanstat::models::sudachi_local().is_cached()
        {
            "Downloading SudachiDict (137 MB, first Japanese use only), then classifying Anki notes…"
        } else {
            "Classifying Anki notes…"
        };
        let s = self.anki.settings.clone();
        let seg = self.anki_seg_config();
        self.anki.built_with = Some(seg.clone());
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let col = col.lock().unwrap_or_else(|e| e.into_inner());
            let _ = tx.send(AnkiMsg::Built(Box::new(col.knowledge(&s, &seg))));
            ctx.request_repaint();
        });
        self.anki.rx = Some(rx);
    }

    /// The user changed the selection or an option: persist it, then re-classify.
    fn anki_changed(&mut self, ctx: &egui::Context) {
        if let Err(e) = self.anki.settings.save() {
            self.anki.error = Some(e);
        }
        self.anki_rebuild(ctx);
        // "Match Traditional ↔ Simplified" also decides how the book's
        // characters are counted.
        if self
            .report
            .as_ref()
            .is_some_and(|r| r.language == Language::Chinese && r.chars_folded != self.fold_chars())
        {
            self.rebuild_report();
        } else {
            self.refresh_anki_coverage();
        }
    }

    /// Count the Traditional and Simplified forms of a Chinese character as
    /// one: the Chinese "Match Traditional ↔ Simplified" option.
    fn fold_chars(&self) -> bool {
        if self.language == Language::Chinese {
            self.anki.settings.match_across_scripts
        } else {
            self.stash.get(&Language::Chinese).map_or_else(
                || {
                    anki::Settings::load_for(Language::Chinese)
                        .0
                        .match_across_scripts
                },
                |d| d.anki.settings.match_across_scripts,
            )
        }
    }

    fn poll_anki(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.anki.rx else { return };
        let msg = match rx.try_recv() {
            Ok(m) => m,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            Err(_) => {
                self.anki.rx = None;
                return;
            }
        };
        self.anki.rx = None;
        self.anki.busy = "";
        match msg {
            AnkiMsg::Opened(Ok(c)) => {
                let before = self.anki.settings.clone();
                self.anki.settings.rebind(&c.info);
                if self.anki.settings != before
                    && let Err(e) = self.anki.settings.save()
                {
                    self.anki.error = Some(e);
                }
                self.anki.info = Some(c.info.clone());
                self.anki.col = Some(Arc::new(Mutex::new(c)));
                self.anki_rebuild(ctx);
            }
            AnkiMsg::Built(b) => match *b {
                Ok(k) => {
                    self.anki.knowledge = Some(Arc::new(k));
                    self.refresh_anki_coverage();
                    if std::mem::take(&mut self.anki.dirty) {
                        self.anki_rebuild(ctx);
                    }
                }
                Err(e) => self.anki.error = Some(e),
            },
            AnkiMsg::Opened(Err(e)) => self.anki.error = Some(e),
        }
    }

    /// Make the known words (Anki, if on, plus the levels, word lists and
    /// books marked known) again, if what they are made from has changed.
    fn update_merged(&mut self) {
        let from = MergedFrom {
            anki: self
                .anki
                .knowledge
                .clone()
                .filter(|_| self.anki.settings.enabled),
            match_across_scripts: self.anki.settings.match_across_scripts,
            known: self.known.settings.clone(),
        };
        if self.known.merged_from.as_ref() == Some(&from) {
            return;
        }
        self.known.merged = read_books::merged_knowledge(&self.anki, &self.known.settings);
        self.known.merged_from = Some(from);
    }

    fn refresh_anki_coverage(&mut self) {
        self.update_merged();
        // Names don't count against the difficulty: the user dictionary plus
        // the recommended Suggested words (once discovery has finished).
        let mut names: HashSet<String> = self.user_words.iter().cloned().collect();
        names.extend(
            self.suggestions
                .iter()
                .flatten()
                .filter(|c| c.recommended)
                .map(|c| c.word.clone()),
        );
        let skip_hapax = self.settings.skip_hapax;
        let Some(r) = &mut self.report else { return };
        self.report_gen += 1;
        let had = r.anki.is_some();
        r.anki = self.known.merged.as_ref().map(|k| {
            BookCoverage::compute(
                k,
                &r.char_freq,
                &r.word_freq,
                &names,
                &r.word_parts,
                skip_hapax,
            )
        });
        if r.anki.is_none() {
            self.anki_filter = AnkiFilter::All;
        } else if !had {
            // The Anki section just appeared on the Overview.
            self.fit_pending = true;
        }
        self.record_history(&names);
    }

    /// Save the Overview's numbers as the book's history entry, replacing
    /// the one from an earlier analysis, and its words for the reading path
    /// when they changed. `names`: as left out of the difficulty.
    fn record_history(&mut self, names: &HashSet<String>) {
        let (Some(r), Some(id), Some(path)) = (&self.report, &self.report_id, &self.path) else {
            return;
        };
        let digest = {
            use std::hash::{Hash, Hasher};
            let mut h = std::hash::DefaultHasher::new();
            id.hash(&mut h);
            r.segmenter.label().hash(&mut h);
            (r.words_total, r.words_unique).hash(&mut h);
            self.included.hash(&mut h);
            let mut n: Vec<&String> = names.iter().collect();
            n.sort_unstable();
            n.hash(&mut h);
            h.finish()
        };
        let profile_path = self.history.profile(id);
        let profile = (self.profile_written.as_ref() != Some(&(id.clone(), digest))
            || !profile_path.is_file())
        .then(|| Profile::from_report(id, r, names));
        let entry = history::Entry {
            id: id.clone(),
            path: std::path::absolute(path).unwrap_or_else(|_| path.clone()),
            title: r
                .title
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| file_stem(path)),
            analysed: history::now(),
            stats: history::Stats::from_report(r),
            sections: Some(r.sections.iter().map(|s| s.included).collect()),
            inputs: String::new(),
        };
        let mut entry = entry;
        entry.inputs = self
            .lang_inputs(entry.stats.language)
            .map(|i| i.joined())
            .unwrap_or_default();
        let touch = std::mem::take(&mut self.history_touch);
        if let Err(e) = self.history.record(entry, touch) {
            self.history_error = Some(e);
        }
        if let Some(p) = profile {
            match p.save(&profile_path) {
                Ok(()) => {
                    self.exists.set(profile_path, true);
                    self.profile_written = Some((p.id, digest));
                    self.profiles_version += 1;
                }
                Err(e) => self.history_error = Some(e),
            }
        }
    }

    fn anki_window(&mut self, ctx: &egui::Context) {
        let mut open = self.anki.window;
        let mut changed = false;
        let mut reopen = false;
        let lang = self.language;
        let title = match lang {
            Language::Chinese => "Anki (中文)",
            Language::Korean => "Anki (한국어)",
            Language::Japanese => "Anki (日本語)",
        };
        egui::Window::new(title)
            .id(egui::Id::unique("anki-window"))
            .open(&mut open)
            .default_size([780.0, 680.0])
            .min_height(420.0)
            .vscroll(false)
            .show(ctx, |ui| {
                let a = &mut self.anki;
                changed |= ui
                    .checkbox(&mut a.settings.enabled, "Show known-vocabulary statistics from Anki")
                    .changed();
                ui.horizontal_wrapped(|ui| {
                    ui.label("Collection:");
                    let current = a.settings.collection.clone();
                    let shown = current
                        .as_ref()
                        .map(|p| {
                            a.profiles
                                .iter()
                                .find(|pr| &pr.collection == p)
                                .map(|pr| format!("Profile: {}", pr.name))
                                .unwrap_or_else(|| p.display().to_string())
                        })
                        .unwrap_or_else(|| "(none)".into());
                    egui::ComboBox::from_id_salt("anki-profile")
                        .selected_text(shown)
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for p in &a.profiles {
                                if ui
                                    .selectable_label(current.as_ref() == Some(&p.collection), &p.name)
                                    .on_hover_text(p.collection.display().to_string())
                                    .clicked()
                                    && current.as_ref() != Some(&p.collection)
                                {
                                    a.settings.collection = Some(p.collection.clone());
                                    reopen = true;
                                }
                            }
                        });
                    if ui.button("Browse…").clicked()
                        && let Some(p) = rfd::FileDialog::new()
                            .add_filter("Anki collection", &["anki2", "anki21"])
                            .pick_file()
                    {
                        a.settings.collection = Some(p);
                        reopen = true;
                    }
                    if ui
                        .add_enabled(a.settings.collection.is_some(), egui::Button::new("⟳ Reload"))
                        .on_hover_text("Re-read the collection, e.g. after reviewing in Anki")
                        .clicked()
                    {
                        reopen = true;
                    }
                    if !a.busy.is_empty() {
                        ui.spinner();
                        ui.label(a.busy);
                    }
                });
                if let Some(e) = &a.error {
                    ui.colored_label(palette::error(), format!("⚠ {e}"));
                }
                if a.profiles.is_empty() && a.settings.collection.is_none() {
                    let base = anki::anki_base()
                        .map_or_else(|| "Anki's data folder".to_owned(), |b| b.display().to_string());
                    ui.label(format!("No Anki profiles found in {base}. Use Browse… to pick a collection.anki2."));
                }
                let Some(info) = a.info.clone() else { return };
                // Options and results are pinned to the bottom; the deck and
                // field lists get whatever height is left and scroll inside it.
                egui::Panel::bottom("anki-bottom")
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin { top: 6, ..Default::default() }))
                    .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    changed |= ui
                        .checkbox(&mut a.settings.include_suspended, "Count suspended cards")
                        .on_hover_text("On: a suspended card counts like any other card. Off: suspended cards are ignored.")
                        .changed();
                    if lang == Language::Chinese {
                        changed |= ui
                            .checkbox(&mut a.settings.match_across_scripts, "Match Traditional ↔ Simplified")
                            .on_hover_text("Fold both your cards and the book to Simplified (character level) before matching.")
                            .changed();
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    changed |= ui
                        .checkbox(&mut a.settings.mature_by_stability, "Mature by FSRS stability")
                        .on_hover_text(format!(
                            "Off: a review card is mature at an interval of {} days or more (Anki's definition). The interval depends on desired retention: \
                             at 90% it is about the stability, above 90% shorter, below 90% longer.\n\
                             On: a review card is mature once its FSRS stability (the days until recall falls to 90%) reaches the value on the slider, \
                             whatever the desired retention. Cards without FSRS data keep the interval rule.",
                            anki::MATURE_IVL
                        ))
                        .changed();
                    let r = ui.add_enabled(
                        a.settings.mature_by_stability,
                        egui::Slider::new(&mut a.settings.mature_stability_days, 1..=365)
                            .logarithmic(true)
                            .prefix("≥ ")
                            .suffix(" days"),
                    );
                    // Reclassify once the drag ends, not on every frame of it.
                    changed |= r.drag_stopped() || (r.changed() && !r.dragged());
                });
                if !a.settings.has_selection() {
                    ui.label(RichText::new("Select at least one deck and one field.").color(palette::warn()));
                } else if let Some(k) = &a.knowledge {
                    let s = &k.summary;
                    ui.add_space(4.0);
                    ui.label(format!(
                        "{} notes{}{}{}{}",
                        fmt_int(s.notes),
                        if s.notes_without_chinese > 0 {
                            format!(
                                " ({} have no {} in the ticked fields)",
                                fmt_int(s.notes_without_chinese),
                                lang.name()
                            )
                        } else {
                            String::new()
                        },
                        if s.suspended_excluded > 0 {
                            format!(", {} suspended cards ignored", fmt_int(s.suspended_excluded))
                        } else {
                            String::new()
                        },
                        if s.no_stability > 0 {
                            format!(
                                ", {} review cards have no FSRS stability and use the {}-day interval",
                                fmt_int(s.no_stability),
                                anki::MATURE_IVL
                            )
                        } else {
                            String::new()
                        },
                        match s.segmenter {
                            Some(k) if s.sentences > 0 => format!(
                                ", {} sentences split into words ({})",
                                fmt_int(s.sentences),
                                k.label()
                            ),
                            _ => String::new(),
                        }
                    ));
                    match lang {
                        Language::Chinese => status_grid(ui, "anki-summary", &[("Cards", &s.cards), ("Words", &s.words), ("Characters", &s.chars)]),
                        Language::Korean => status_grid(ui, "anki-summary", &[("Cards", &s.cards), ("Words", &s.words)]),
                        Language::Japanese => status_grid(ui, "anki-summary", &[("Cards", &s.cards), ("Words", &s.words), ("Kanji", &s.chars)]),
                    }
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "{0} Learning and relearning cards are not counted as known. \
                         {2} HTML, sound tags and [readings] are ignored. \
                         The collection is read from a temporary copy and never modified. Choices are saved to {1}, separately for each language.",
                        match a.settings.stability_threshold() {
                            None => format!(
                                "Young = review interval under {0} days, mature = {0}+ days (Anki's definitions).",
                                anki::MATURE_IVL
                            ),
                            Some(t) => format!(
                                "Mature = review card with FSRS stability of {t}+ days, young = below that (cards without FSRS data: interval of {}+ days).",
                                anki::MATURE_IVL
                            ),
                        },
                        anki::Settings::path_for(lang).display(),
                        match lang {
                            Language::Chinese => "A ticked field holding a word counts as that word; one holding a sentence is split into words with the book's segmenter.",
                            Language::Korean => "Every ticked field is analysed with Kiwi and counts as the dictionary forms of its words (먹었어요 → 먹다), like the book.",
                            Language::Japanese => "Every ticked field is analysed with Sudachi and counts as its words in dictionary form with spellings merged (わかった, 分かる → 分かる), like the book; furigana readings (会[あ]う) are ignored.",
                        }
                    ))
                    .weak()
                    .small(),
                );
                    });
                egui::CentralPanel::no_frame().show(ui, |ui| {
                ui.separator();
                ui.columns(2, |cols| {
                    let ui = &mut cols[0];
                    ui.strong("Decks");
                    ui.label(RichText::new("A deck includes its subdecks.").weak());
                    egui::ScrollArea::vertical()
                        .id_salt("anki-decks")
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for d in &info.decks {
                                ui.horizontal(|ui| {
                                    ui.add_space(d.depth() as f32 * 18.0);
                                    let mut v = a.settings.deck_selected(d);
                                    if ui
                                        .checkbox(&mut v, format!("{}  ({} cards)", d.leaf(), d.cards))
                                        .on_hover_text(&d.name)
                                        .changed()
                                    {
                                        a.settings.set_deck(d, v);
                                        a.settings.enabled = true;
                                        changed = true;
                                    }
                                });
                            }
                        });

                    let ui = &mut cols[1];
                    ui.horizontal(|ui| {
                        ui.strong("Note types → fields");
                        ui.checkbox(&mut a.show_unused_types, "show unused");
                    });
                    ui.label(RichText::new("Tick the fields that hold the word (not sentences).").weak());
                    egui::ScrollArea::vertical()
                        .id_salt("anki-types")
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for nt in info
                                .note_types
                                .iter()
                                .filter(|n| n.notes > 0 || a.show_unused_types)
                            {
                                let n_sel = nt
                                    .fields
                                    .iter()
                                    .filter(|f| a.settings.field_selected(nt, f))
                                    .count();
                                let title = if n_sel > 0 {
                                    RichText::new(format!("{}  ({} notes, {n_sel} field(s))", nt.name, nt.notes)).strong()
                                } else {
                                    RichText::new(format!("{}  ({} notes)", nt.name, nt.notes))
                                };
                                egui::CollapsingHeader::new(title)
                                    .id_salt(("anki-nt", nt.id))
                                    .default_open(n_sel > 0)
                                    .show(ui, |ui| {
                                        for f in &nt.fields {
                                            let mut v = a.settings.field_selected(nt, f);
                                            if ui.checkbox(&mut v, f).changed() {
                                                a.settings.set_field(nt, f, v);
                                                a.settings.enabled = true;
                                                changed = true;
                                            }
                                        }
                                    });
                            }
                        });
                });
                });
            });
        self.anki.window = open;
        if reopen {
            if let Err(e) = self.anki.settings.save() {
                self.anki.error = Some(e);
            }
            self.anki_open(ctx);
        } else if changed {
            self.anki_changed(ctx);
        }
    }

    fn poll_suggestions(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.suggest_rx else { return };
        match rx.try_recv() {
            Ok(v) => {
                self.selected = v
                    .iter()
                    .filter(|c| c.recommended)
                    .map(|c| c.word.clone())
                    .collect();
                // The Overview's "likely names" banner appears above the content.
                if !self.selected.is_empty() {
                    self.fit_pending = true;
                }
                self.suggestions = Some(v);
                self.suggest_rx = None;
                // The recommended ones are left out of the difficulty.
                self.refresh_anki_coverage();
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100))
            }
            Err(_) => self.suggest_rx = None,
        }
    }

    /// Add the selected suggestions to the forced words and re-segment.
    fn apply_selected(&mut self, ctx: &egui::Context) {
        let mut words = self.user_words.clone();
        let have: HashSet<String> = words.iter().cloned().collect();
        let mut new: Vec<String> = self
            .selected
            .iter()
            .filter(|w| !have.contains(*w))
            .cloned()
            .collect();
        new.sort();
        words.extend(new);
        self.set_user_words(words, ctx);
    }

    /// Replace the forced words, save them, and re-segment the open book.
    fn set_user_words(&mut self, words: Vec<String>, ctx: &egui::Context) {
        if words == self.user_words {
            return;
        }
        self.user_words = words;
        self.dict_error = hanstat::save_user_dict(self.language, &self.user_words).err();
        self.rerun(ctx, false);
    }

    /// Words typed in the "Force a word" box: separated by spaces, commas or
    /// 、, each needing at least one non-punctuation character.
    fn parse_force_input(s: &str) -> Vec<String> {
        s.split(|c: char| c.is_whitespace() || matches!(c, ',' | '，' | '、' | ';' | '；'))
            .map(str::trim)
            .filter(|w| w.chars().any(char::is_alphanumeric))
            .map(str::to_owned)
            .collect()
    }

    /// Forced words: add by typing, remove one by one or all. Shared by the
    /// Suggested words tab and the Forced words window.
    fn forced_words_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let busy = self.job.is_some();
        let mut words = self.user_words.clone();
        ui.horizontal(|ui| {
            ui.label("Force a word:");
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.force_input)
                    .hint_text("蒋舸")
                    .desired_width(160.0),
            );
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let new = Self::parse_force_input(&self.force_input);
            let add = ui
                .add_enabled(!busy && !new.is_empty(), egui::Button::new("Add"))
                .on_hover_text("Always count it as one word, in every book of this language. Several words can be separated by spaces.")
                .clicked();
            if (add || enter) && !busy && !new.is_empty() {
                for w in new {
                    if !words.contains(&w) {
                        words.push(w);
                    }
                }
                self.force_input.clear();
                if enter {
                    r.request_focus();
                }
            }
            if !words.is_empty()
                && ui
                    .add_enabled(!busy, egui::Button::new("Remove all"))
                    .on_hover_text("Remove every forced word of this language (also from the saved list)")
                    .clicked()
            {
                words.clear();
            }
        });
        if !self.user_words.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{} forced:", self.user_words.len()));
                for w in &self.user_words {
                    let b = egui::Button::new(format!("{w} ✖")).small();
                    if ui
                        .add_enabled(!busy, b)
                        .on_hover_text("Stop forcing this word")
                        .clicked()
                    {
                        words.retain(|x| x != w);
                    }
                }
            });
        }
        if let Some(e) = &self.dict_error {
            ui.colored_label(palette::error(), e);
        }
        self.set_user_words(words, ctx);
    }

    fn forced_window(&mut self, ctx: &egui::Context) {
        let mut open = self.forced_window;
        egui::Window::new("Forced words")
            .id(egui::Id::unique("forced-window"))
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label(
                    "Each forced word always counts as one word, in every book of this language: use it for a name the \
                     segmenter splits or glues to its neighbours (蒋舸没, 蒋舸问). Saved between runs.",
                );
                ui.add_space(4.0);
                self.forced_words_ui(ui, ctx);
            });
        self.forced_window &= open;
    }

    /// Status-bar part of the updater: download progress, then a restart button.
    fn update_status(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        match self.updater.phase() {
            UpdatePhase::Downloading { tag, done, total } => {
                ui.separator();
                ui.spinner();
                ui.label(format!(
                    "Downloading hanstat {tag} ({:.0} of {:.0} MB)",
                    done as f64 / 1e6,
                    total as f64 / 1e6
                ));
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                        .desired_width(160.0)
                        .show_percentage(),
                );
            }
            UpdatePhase::Available(rel) => {
                ui.separator();
                if ui
                    .button(format!("⬆ Update to {}", rel.tag))
                    .on_hover_text(notes_preview(&rel))
                    .clicked()
                {
                    self.updater.install(ctx, rel);
                }
            }
            UpdatePhase::Installed(rel) => {
                ui.separator();
                let label =
                    RichText::new(format!("⟳ Restart to use {}", rel.tag)).color(palette::good());
                let hover = format!(
                    "hanstat {} is installed and starts next time. Restart now to use it; the open file is reopened.\n\n{}",
                    rel.tag,
                    notes_preview(&rel)
                );
                if ui.button(label).on_hover_text(hover).clicked() {
                    self.restart(ctx);
                }
            }
            UpdatePhase::Failed(e) => {
                ui.separator();
                ui.colored_label(palette::warn(), "⚠ Update failed")
                    .on_hover_text(e);
            }
            UpdatePhase::Idle | UpdatePhase::Checking | UpdatePhase::UpToDate => {}
        }
    }

    /// The version label at the right of the status bar, with update settings.
    fn version_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.menu_button(
            RichText::new(update::version_string()).weak(),
            |ui| {
                if let Some(reason) = &self.updater.disabled {
                    ui.label(RichText::new(reason).weak());
                } else {
                    if ui
                        .checkbox(
                            &mut self.updater.settings.auto_update,
                            "Install updates automatically",
                        )
                        .on_hover_text("Check GitHub for a new release at startup and install it in the background. It runs after a restart.")
                        .changed()
                        && let Err(e) = self.updater.settings.save()
                    {
                        self.set_error("settings", e);
                    }
                    let busy = self.updater.busy();
                    if ui
                        .add_enabled(!busy, egui::Button::new("Check for updates now"))
                        .clicked()
                    {
                        self.updater.check(ctx, self.updater.settings.auto_update);
                    }
                    match self.updater.phase() {
                        UpdatePhase::Idle => {}
                        UpdatePhase::Checking => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Checking…");
                            });
                        }
                        UpdatePhase::UpToDate => {
                            ui.label("hanstat is up to date.");
                        }
                        UpdatePhase::Available(rel) => {
                            ui.label(format!("{} is available.", rel.tag));
                        }
                        UpdatePhase::Downloading { tag, .. } => {
                            ui.label(format!("Downloading {tag}…"));
                        }
                        UpdatePhase::Installed(rel) => {
                            ui.label(format!("{} is installed; restart to use it.", rel.tag));
                        }
                        UpdatePhase::Failed(e) => {
                            ui.colored_label(palette::warn(), e);
                        }
                    }
                }
                ui.separator();
                ui.hyperlink_to("Releases on GitHub", update::RELEASES_PAGE);
            },
        );
    }

    /// Whether writing-style ratings are on (and can run here).
    fn style_on(&self) -> bool {
        self.settings.writing_style && style::supported()
    }

    /// Percentage points a book's writing style takes off its share of
    /// running words known for its difficulty, and adds to its target in the
    /// reading path; 0 while ratings are off or the book isn't rated.
    fn style_offset(&self, id: &str) -> f64 {
        if !self.style_on() {
            return 0.0;
        }
        self.style.store.get(id).map_or(0.0, |r| {
            style::target_offset(r.score, self.settings.style_weight)
        })
    }

    /// While ratings are on: start the rater, keep what it rated, and queue
    /// the Chinese books without a rating, the open one first.
    fn style_tick(&mut self, ctx: &egui::Context) {
        if !self.style_on() {
            return;
        }
        let st = &mut self.style;
        let rater = st.rater.get_or_insert_with(|| {
            let c = ctx.clone();
            style::Rater::new(move || c.request_repaint())
        });
        let mut changed = false;
        for o in rater.take() {
            match o {
                style::Outcome::Rated(id, r) => {
                    st.skipped.remove(&id);
                    st.store.insert(id, r);
                    changed = true;
                }
                style::Outcome::Skipped(id, why) => {
                    st.skipped.insert(id, why);
                }
            }
        }
        if changed {
            st.version += 1;
            st.error = st.store.save().err();
        }
        if let (Some(id), Some(r), Some(path)) = (&self.report_id, &self.report, &self.path)
            && r.language == Language::Chinese
            && st.store.get(id).is_none()
            && st.asked.insert(id.clone())
        {
            let title = r
                .title
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| file_stem(path));
            rater.push(
                style::Job {
                    id: id.clone(),
                    path: path.clone(),
                    title,
                },
                true,
            );
        }
        for e in &self.history.entries {
            if e.stats.language != Language::Chinese
                || st.store.get(&e.id).is_some()
                || !st.asked.insert(e.id.clone())
            {
                continue;
            }
            if e.path.is_file() {
                rater.push(
                    style::Job {
                        id: e.id.clone(),
                        path: e.path.clone(),
                        title: e.title.clone(),
                    },
                    false,
                );
            } else {
                st.skipped
                    .insert(e.id.clone(), "The file is no longer where it was.".into());
            }
        }
    }

    /// What the writing-style rater is doing, in the status bar.
    fn style_status(&self, ui: &mut egui::Ui) {
        let Some(r) = self.style.rater.as_ref().filter(|_| self.style_on()) else {
            return;
        };
        match r.phase() {
            style::Phase::Idle => {}
            style::Phase::Downloading { done, total } => {
                ui.separator();
                ui.spinner();
                ui.label(format!(
                    "Downloading the writing-style model ({:.0} of {:.0} MB, first use only)",
                    done as f64 / 1e6,
                    total as f64 / 1e6
                ));
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                        .desired_width(160.0)
                        .show_percentage(),
                );
            }
            style::Phase::Starting => {
                ui.separator();
                ui.spinner();
                ui.label("Loading the writing-style model…");
            }
            style::Phase::Rating { title, left } => {
                ui.separator();
                ui.spinner();
                ui.label(if left > 0 {
                    format!("Rating the writing style of {title} ({left} more to go)")
                } else {
                    format!("Rating the writing style of {title}")
                });
            }
            style::Phase::Failed(e) => {
                ui.separator();
                ui.colored_label(palette::warn(), "⚠ Writing-style rating failed")
                    .on_hover_text(e);
            }
        }
    }

    /// The Settings window, opened with ⚙ at the right of the status bar.
    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_window;
        egui::Window::new("Settings")
            .id(egui::Id::unique("settings-window"))
            .open(&mut open)
            .default_width(460.0)
            .resizable(false)
            .show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.strong("Counting");
            if ui
                .checkbox(
                    &mut self.settings.skip_hapax,
                    "Leave out words that occur only once",
                )
                .changed()
            {
                if let Err(e) = self.settings.save() {
                    self.set_error("settings", e);
                }
                // The open book now; the reading path and Library statistics
                // rebuild on their own (their key has the setting), and the
                // other books' rows are updated in the background.
                self.refresh_anki_coverage();
            }
            ui.label(
                RichText::new(
                    "Words that occur once in a book (many are rare words, typos or segmenter splits you'd guess or skip) \
                     don't count toward the words you know, the difficulty or the reading path, known or not. \
                     The word counts and coverage table still include them.",
                )
                .weak()
                .small(),
            );
            ui.separator();
            ui.strong("Reading speed");
            egui::Grid::new("speeds").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                let s = &mut self.settings;
                let mut changed = false;
                ui.label("Chinese");
                changed |= ui
                    .add(egui::DragValue::new(&mut s.cpm).range(50.0..=2000.0).speed(5.0).suffix(" chars/min"))
                    .on_hover_text("Chinese characters per minute")
                    .changed();
                ui.end_row();
                ui.label("Japanese");
                changed |= ui
                    .add(egui::DragValue::new(&mut s.ja_cpm).range(50.0..=2000.0).speed(5.0).suffix(" chars/min"))
                    .on_hover_text("Kanji and kana per minute")
                    .changed();
                ui.end_row();
                ui.label("Korean");
                changed |= ui
                    .add(egui::DragValue::new(&mut s.wpm).range(20.0..=1000.0).speed(2.0).suffix(" words/min"))
                    .on_hover_text("Words per minute, as counted here: dictionary forms of content words")
                    .changed();
                ui.end_row();
                if changed {
                    self.settings_unsaved = true;
                }
            });
            ui.label(
                RichText::new(
                    "For the reading times in the Library and on each book's Overview, where a book's speed can be changed on its own to try another.",
                )
                .weak()
                .small(),
            );
            ui.separator();
            ui.strong("Writing style");
            let supported = style::supported();
            let mut on = self.settings.writing_style;
            ui.add_enabled(
                supported,
                egui::Checkbox::new(&mut on, "Rate writing style with a local AI model"),
            )
            .on_disabled_hover_text(
                "llama.cpp, which runs the model, has no build for this platform (only Windows x64, macOS on Apple silicon and Linux x86-64).",
            );
            if on != self.settings.writing_style {
                self.settings.writing_style = on;
                if let Err(e) = self.settings.save() {
                    self.set_error("settings", e);
                }
                if !on {
                    self.style.stop();
                }
            }
            ui.label(
                RichText::new(
                    "Chinese books only. Rates how hard a book's prose is to read when you know its words \
                     (sentence structure, written and classical register, rhetoric and description, what you must infer) \
                     from four samples, shown as a percentage of the rated books in your Library (50% is their median) and a word: plain, typical, literary or very literary. \
                     It counts toward each book's difficulty, with the words you know, and toward the reading path.",
                )
                .weak()
                .small(),
            );
            let dl = style::download_size();
            if dl > 0 {
                ui.label(
                    RichText::new(format!(
                        "First use downloads llama.cpp and the Qwen3.5-4B model ({:.1} GB). They run on this computer, \
                         on the GPU if it has one (about 5 seconds a book), otherwise on the CPU (much longer); the books never leave it.",
                        dl as f64 / 1e9
                    ))
                    .weak()
                    .small(),
                );
            }
            if !self.style_on() {
                return;
            }
            let hover = format!(
                "The weight works on the model's own rating (1 plain – 9 near classical Chinese, shown with ⓘ on a book's \
                 Difficulty tile), not on the percentage, which only compares the books of your Library. A book rated a point \
                 above typical ({:.2}) counts as this many points less of its running words known, for its difficulty and in the \
                 reading path (whose target rises by as much); one rated below typical, as more. \
                 0 shows the rating without counting it. Not measured: {} is a judgment.",
                style::TYPICAL,
                style::DEFAULT_WEIGHT
            );
            ui.horizontal(|ui| {
                ui.label("Weight").on_hover_text(&hover);
                if ui
                    .add(
                        egui::DragValue::new(&mut self.settings.style_weight)
                            .range(0.0..=10.0)
                            .speed(0.05)
                            .max_decimals(1)
                            .suffix(" % known per point of the model's rating"),
                    )
                    .on_hover_text(&hover)
                    .changed()
                    && let Err(e) = self.settings.save()
                {
                    self.set_error("settings", e);
                }
            });
            ui.separator();
            let chinese: Vec<&history::Entry> = self
                .history
                .entries
                .iter()
                .filter(|e| e.stats.language == Language::Chinese)
                .collect();
            let rated = chinese.iter().filter(|e| self.style.store.get(&e.id).is_some()).count();
            ui.label(format!(
                "{rated} of the {} Chinese books in the Library are rated.",
                chinese.len()
            ));
            if let Some(mode) = self
                .style
                .rater
                .as_ref()
                .map(|r| r.mode())
                .filter(|&m| m != style::Mode::Gpu)
            {
                ui.label(
                    RichText::new(format!(
                        "llama.cpp runs {}: it crashed with faster settings on this computer.",
                        mode.describe()
                    ))
                    .weak(),
                )
                .on_hover_text(
                    "Kept until llama.cpp is updated. The log has the crash. \
                     Seen with AMD's Vulkan driver on an RX 7600 XT.",
                );
            }
            let skipped: Vec<String> = chinese
                .iter()
                .filter_map(|e| self.style.skipped.get(&e.id).map(|w| format!("{}: {w}", e.title)))
                .collect();
            if !skipped.is_empty() {
                ui.label(RichText::new(format!("{} could not be rated.", skipped.len())).weak())
                    .on_hover_text(skipped.join("\n"));
            }
            let phase = self.style.rater.as_ref().map(|r| r.phase());
            let failed = matches!(phase, Some(style::Phase::Failed(_)));
            if let Some(style::Phase::Failed(e)) = &phase {
                ui.colored_label(palette::warn(), e);
            }
            let unrated = chinese.len() - rated;
            // Busy: the books without a rating are already waiting their turn.
            // (Downloading or starting counts too: stopping then would wait for it.)
            let busy = !failed
                && self.style.rater.as_ref().is_some_and(|r| {
                    r.pending() > 0 || !matches!(phase, Some(style::Phase::Idle))
                });
            let mut again = false;
            ui.horizontal(|ui| {
                again = ui
                    .add_enabled(
                        unrated > 0 && !busy,
                        egui::Button::new(if unrated == 1 {
                            "Rate the book without a rating".to_owned()
                        } else {
                            format!("Rate the {unrated} books without a rating")
                        }),
                    )
                    .on_hover_text(
                        "Start the rater afresh, with a new log, and try every Chinese book in the Library \
                         that has no rating yet, those that failed included.",
                    )
                    .on_disabled_hover_text(if busy {
                        "Still rating: the books without a rating are waiting their turn."
                    } else {
                        "Every Chinese book in the Library is rated."
                    })
                    .clicked();
                let log = style::Log::path();
                if log.is_file()
                    && ui
                        .button("Open the log")
                        .on_hover_text(format!(
                            "What the rater did in its last run, and why a book failed: {}",
                            log.display()
                        ))
                        .clicked()
                    && let Err(e) = open_with_system(&log)
                {
                    self.style.error = Some(e);
                }
            });
            if again {
                self.style.stop();
                self.style.error = None;
            }
            if let Some(e) = &self.style.error {
                ui.colored_label(palette::warn(), e);
            }
        });
        self.settings_window &= open;
    }

    /// Start the (now updated) exe on the same file and tab, then close.
    fn restart(&mut self, ctx: &egui::Context) {
        // The new copy reads the Library and known words: write them first.
        self.finish_removal();
        hanstat::saver::flush();
        let exe = match update::exe_path() {
            Ok(p) => p,
            Err(e) => {
                self.set_error("restart", e);
                return;
            }
        };
        let mut cmd = std::process::Command::new(exe);
        if let Some(p) = &self.path {
            cmd.arg(p);
        }
        cmd.args(["--tab", self.tab.arg()]);
        match cmd.spawn() {
            Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(e) => self.set_error("restart", format!("Cannot start {}: {e}", exe.display())),
        }
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        if let Ok(JobMsg::Done(res)) = job.rx.try_recv() {
            self.job = None;
            match res {
                Ok(l) => {
                    let same_book = self.book.as_ref().is_some_and(|b| Arc::ptr_eq(b, &l.book));
                    if !same_book || self.included.len() != l.book.sections.len() {
                        // The sections chosen when the book was last analysed, if any.
                        let n = l.book.sections.len();
                        let saved = l.file_id.as_ref().and_then(|id| {
                            self.history
                                .entries
                                .iter()
                                .find(|e| e.id == *id)
                                .and_then(|e| e.sections_for(n))
                                .map(<[bool]>::to_vec)
                        });
                        self.included = saved.unwrap_or_else(|| {
                            l.book
                                .sections
                                .iter()
                                .map(|s| s.default_included())
                                .collect()
                        });
                    }
                    if !same_book {
                        (self.cpm, self.ja_cpm, self.wpm) = (None, None, None);
                    }
                    let lang = l.prep.language;
                    self.cache_usage = self.cache.as_ref().map_or((0, 0), Cache::usage);
                    self.book = Some(l.book);
                    self.prep = Some(l.prep);
                    // The old report is not this book's: drop it before the
                    // language switch recomputes its coverage (and records it).
                    self.report = None;
                    self.report_id = None;
                    self.book_id = l.file_id;
                    self.history_touch = true;
                    // The book's language decides which dictionary, Anki selection and word lists apply.
                    self.set_language(lang, ctx);
                    self.rebuild_report();
                    self.fit_pending = true;
                    // Anki sentences must be segmented like this book.
                    if self.anki.col.is_some()
                        && self.anki.built_with.as_ref() != Some(&self.anki_seg_config())
                    {
                        self.anki_rebuild(ctx);
                    }
                }
                // Replaced by another analysis: nothing to show.
                Err(e) if e == analyze::CANCELLED => {}
                Err(e) => self.set_error("open", e),
            }
        }
    }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(
            storage,
            UI_KEY,
            &UiState {
                tab: self.tab,
                history_tab: self.history_tab,
                history_sort: self.history_sort,
                library_hide_read: self.library_hide_read,
                char_sort: self.char_sort,
                word_sort: self.word_sort,
            },
        );
    }

    fn on_exit(&mut self) {
        self.finish_removal();
        hanstat::saver::flush();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        palette::set_dark(ui.visuals().dark_mode);
        let now = ctx.input(|i| i.time);
        self.poll_job(&ctx);
        self.poll_suggestions(&ctx);
        self.start_discovery(&ctx);
        self.poll_anki(&ctx);
        self.poll_plan(&ctx);
        self.poll_stats(&ctx);
        // Counted only while shown: it reads every book's word data.
        if self.show_history && self.history_tab == HistTab::Stats {
            self.refresh_stats(&ctx);
        }
        self.poll_batch(&ctx);
        self.auto_refresh(&ctx);
        if self.show_history {
            let hist = &self.history;
            self.exists.refresh(
                || {
                    hist.entries
                        .iter()
                        .flat_map(|e| [e.path.clone(), hist.profile(&e.id)])
                        .collect()
                },
                now,
                &ctx,
            );
        }
        if let Some(r) = &self.removed {
            if now >= r.until {
                self.finish_removal();
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(r.until - now));
            }
        }
        if let Some(e) = hanstat::saver::take_error() {
            self.set_error("save", e);
        }
        // A window resized by hand keeps its size: the Overview stops fitting it.
        let inner = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size()));
        if let (Some(was), Some(is)) = (self.last_inner, inner)
            && (was - is).length() > 1.0
            && now >= self.fit_until
        {
            self.auto_fit = false;
        }
        self.last_inner = inner;
        if self.settings_unsaved && !ctx.input(|i| i.pointer.any_down()) {
            self.settings_unsaved = false;
            if let Err(e) = self.settings.save() {
                self.set_error("settings", e);
            }
        }
        self.style_tick(&ctx);

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() {
            self.open_files(dropped, &ctx);
        }

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("📂 Open…")
                    .on_hover_text("Open a book, or select several: each is analysed and added to the Library, and the last one is shown")
                    .clicked()
                    && let Some(files) = rfd::FileDialog::new()
                        .add_filter("E-books and text", &["epub", "txt", "text"])
                        .add_filter("All files", &["*"])
                        .pick_files()
                {
                    self.open_files(files, &ctx);
                }
                ui.separator();
                ui.label("Segmenter:");
                if self.language != Language::Chinese {
                    let k = SegmenterKind::Auto.resolve_in(self.language, hanstat::script::Variant::Unknown);
                    ui.label(RichText::new(k.label()).strong())
                        .on_hover_text(k.description());
                } else {
                    let before = self.segmenter;
                    egui::ComboBox::from_id_salt("seg").selected_text(self.segmenter.label()).show_ui(ui, |ui| {
                        for k in SegmenterKind::ALL {
                            ui.selectable_value(&mut self.segmenter, k, k.label()).on_hover_text(k.description());
                        }
                    });
                    // While a book is being analysed, that analysis stops
                    // and the book is analysed again with the new choice.
                    if before != self.segmenter {
                        self.rerun(&ctx, false);
                    }
                }

                {
                    let hover = match self.language {
                        Language::Chinese => "Words always segmented as single words, saved between runs. Add them here or from the Suggested words tab.",
                        Language::Korean => "Kiwi keeps these whole, as proper nouns. Saved between runs, separately from the Chinese ones.",
                        Language::Japanese => "Sudachi keeps these whole, as proper nouns. Saved between runs, separately from the Chinese and Korean ones.",
                    };
                    if ui
                        .selectable_label(self.forced_window, format!("📖 {} forced words", self.user_words.len()))
                        .on_hover_text(hover)
                        .clicked()
                    {
                        self.forced_window = !self.forced_window;
                    }
                }

                ui.separator();
                let anki_label = match &self.anki.knowledge {
                    Some(k) if self.anki.settings.enabled => format!(
                        "🃏 Anki: {} known words",
                        fmt_int(k.summary.words.known())
                    ),
                    _ => "🃏 Anki…".to_owned(),
                };
                if ui
                    .selectable_label(self.anki.window, anki_label)
                    .on_hover_text("Compare the book with your Anki cards (young and mature)")
                    .clicked()
                {
                    self.anki.window = !self.anki.window;
                    if self.anki.window && self.anki.col.is_none() && self.anki.rx.is_none() {
                        self.anki_open_default(&ctx);
                    }
                }

                let is_txt = self.book.as_ref().is_some_and(|b| b.format == "TXT");
                if is_txt {
                    ui.separator();
                    ui.label("Encoding:");
                    let before = self.encoding;
                    egui::ComboBox::from_id_salt("enc").selected_text(self.encoding.label()).show_ui(ui, |ui| {
                        for e in EncodingChoice::ALL {
                            ui.selectable_value(&mut self.encoding, e, e.label());
                        }
                    });
                    if before != self.encoding {
                        self.rerun(&ctx, true);
                    }
                }

                if self.report.is_some() {
                    ui.separator();
                    ui.menu_button("💾 Export", |ui| {
                        let stem = self.path.as_deref().map(file_stem).unwrap_or_else(|| "hanstat".into());
                        let r = self.report.as_ref().unwrap();
                        let mut msg = None;
                        // A Korean book has no character statistics; a Japanese one counts kanji.
                        let chinese = r.language != Language::Korean;
                        // Japanese word keys are shown as the book spells them.
                        let shown: Vec<(&str, u32)> = r.word_freq.iter().map(|(w, n)| (r.display(w), *n)).collect();
                        if chinese && ui.button("Character frequency (CSV)…").clicked() {
                            msg = save(&format!("{stem}-characters.csv"), "csv", analyze::freq_csv(&r.char_freq, "character"));
                        }
                        if ui.button("Word frequency (CSV)…").clicked() {
                            msg = save(&format!("{stem}-words.csv"), "csv", analyze::freq_csv(&shown, "word"));
                        }
                        if ui.button("Full report (JSON)…").clicked() {
                            msg = save(&format!("{stem}-report.json"), "json", serde_json::to_string_pretty(r).unwrap());
                        }
                        if let Some(c) = &r.anki
                            && ui.button("Words not known, by frequency (TXT)…").clicked()
                        {
                            let list: Vec<&str> = r
                                .word_freq
                                .iter()
                                .zip(&c.word_status)
                                .filter(|(_, s)| !s.is_known())
                                .map(|((w, _), _)| r.display(w))
                                .collect();
                            msg = save(&format!("{stem}-unknown-words.txt"), "txt", list.join("\n") + "\n");
                        }
                        if !self.user_words.is_empty() && ui.button("Forced words as user dictionary (TXT)…").clicked() {
                            msg = save(&format!("{stem}-dictionary.txt"), "txt", self.user_words.join("\n") + "\n");
                        }
                        ui.separator();
                        if chinese && ui.button("Copy unique characters").clicked() {
                            ctx.copy_text(r.char_freq.iter().map(|x| x.0).collect());
                            msg = Some(format!("Copied {} characters", r.char_freq.len()));
                        }
                        if ui.button("Copy unique words (one per line)").clicked() {
                            ctx.copy_text(shown.iter().map(|x| x.0).collect::<Vec<_>>().join("\n"));
                            msg = Some(format!("Copied {} words", r.word_freq.len()));
                        }
                        if let Some(m) = msg {
                            self.toast = Some((m, ui.input(|i| i.time) + 3.0));
                            ui.close();
                        }
                    });
                }

                ui.separator();
                let label = match (self.known.settings.is_empty(), self.language) {
                    (true, Language::Chinese) => "📚 HSK, TOCFL & word lists…".to_owned(),
                    (true, Language::Korean) => "📚 Levels & word lists…".to_owned(),
                    (true, Language::Japanese) => "📚 JLPT & word lists…".to_owned(),
                    (false, _) => format!("📚 Known: {}", self.known.settings.label()),
                };
                let hover = match self.language {
                    Language::Chinese => "Mark HSK 3.0 or TOCFL levels and imported word lists as known, in addition to your Anki cards",
                    Language::Korean => "Mark vocabulary levels 1–6급 and imported Korean word lists as known, in addition to your Anki cards",
                    Language::Japanese => "Mark JLPT levels N5–N1 and imported Japanese word lists as known, in addition to your Anki cards",
                };
                if ui
                    .selectable_label(self.known.window, label)
                    .on_hover_text(hover)
                    .clicked()
                {
                    self.known.window = !self.known.window;
                }

                ui.separator();
                if ui
                    .selectable_label(
                        self.show_history,
                        format!("📖 Library ({})", self.history.entries.len()),
                    )
                    .on_hover_text("Every book analysed before, with its statistics, and a reading path through them")
                    .clicked()
                {
                    self.show_history = !self.show_history;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.mode_badge_ui(ui, &ctx);
                });
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            // The version, ⚙ and messages at the right; the rest fills the
            // space left of them, on more lines when it doesn't fit on one.
            // Both start as one row's height: a layout given the panel's whole
            // height lets separators and centred items fill it, and the panel,
            // sized to its contents, grows with every frame.
            let row = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
            ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.version_menu(ui, &ctx);
                if ui
                    .selectable_label(self.settings_window, RichText::new("⚙").weak())
                    .on_hover_text("Settings")
                    .clicked()
                {
                    self.settings_window = !self.settings_window;
                }
                if let Some((m, until)) = &self.toast {
                    if now < *until {
                        ui.label(RichText::new(m).strong());
                        ctx.request_repaint_after(std::time::Duration::from_millis(250));
                    } else {
                        self.toast = None;
                    }
                }
                let mut undo = false;
                if let Some(r) = &self.removed {
                    undo = ui
                        .button("↩ Undo")
                        .on_hover_text("Put it back in the Library")
                        .clicked();
                    ui.label(format!("Removed “{}” from the Library.", r.entry.title));
                }
                if undo {
                    self.undo_removal();
                }
                // The space left of them, in that row (not the panel's height).
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true), |ui| {
                if self.batch.is_some() {
                    self.batch_status(ui);
                    ui.separator();
                }
                if let Some(job) = &self.job {
                    ui.spinner();
                    let (d, t) = (
                        job.done.load(Ordering::Relaxed),
                        job.total.load(Ordering::Relaxed),
                    );
                    let stage = job.stage.lock().unwrap().clone();
                    if t > 0 {
                        ui.label(stage);
                        ui.add(
                            egui::ProgressBar::new(d as f32 / t as f32)
                                .desired_width(260.0)
                                .show_percentage(),
                        );
                    } else {
                        ui.label(stage);
                    }
                } else if let Some(p) = &self.path {
                    ui.label(file_name(p)).on_hover_text(p.display().to_string());
                } else if drop_works() {
                    ui.label("Open or drop an .epub or .txt file.");
                } else {
                    ui.label("Open an .epub or .txt file.");
                }
                if let Some(f) = &self.models {
                    if !f.finished.load(Ordering::Relaxed) {
                        let (d, t) = (
                            f.done.load(Ordering::Relaxed),
                            f.total.load(Ordering::Relaxed),
                        );
                        ui.separator();
                        ui.spinner();
                        ui.label(format!(
                            "Downloading {} ({:.0} of {:.0} MB, first run only)",
                            *f.current.lock().unwrap(),
                            d as f64 / 1e6,
                            t as f64 / 1e6
                        ));
                        ui.add(
                            egui::ProgressBar::new(d as f32 / t.max(1) as f32)
                                .desired_width(160.0)
                                .show_percentage(),
                        );
                    } else {
                        let errors = f.errors.lock().unwrap();
                        if !errors.is_empty() {
                            ui.separator();
                            ui.colored_label(
                                palette::warn(),
                                "⚠ Model download failed; it will be retried when the model is needed",
                            )
                            .on_hover_text(errors.join("\n\n"));
                        }
                    }
                }
                self.style_status(ui);
                self.update_status(ui, &ctx);
                });
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if !self.errors.is_empty() {
                let mut dismiss = None;
                for (i, (_, e)) in self.errors.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("✖").on_hover_text("Dismiss").clicked() {
                            dismiss = Some(i);
                        }
                        ui.colored_label(palette::error(), format!("⚠ {e}"));
                    });
                }
                if let Some(i) = dismiss {
                    self.errors.remove(i);
                }
                ui.separator();
            }
            if self.show_history {
                self.history_view(ui, &ctx);
                return;
            }
            if self.report.is_none() {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new(if self.job.is_some() || self.batch.is_some() {
                            "Analysing…"
                        } else if drop_works() {
                            "Drop .epub or .txt files here, or click Open…"
                        } else {
                            "Click Open… to choose .epub or .txt files"
                        })
                        .size(22.0)
                        .weak(),
                    );
                });
                return;
            }
            // A Korean book has word statistics only; Korean and Japanese
            // books have no suggested words.
            let lang = self.report.as_ref().map(|r| r.language);
            let korean = lang == Some(Language::Korean);
            let no_suggestions = lang.is_some_and(|l| l != Language::Chinese);
            let hidden = |t: Tab| {
                (korean && t == Tab::Characters) || (no_suggestions && t == Tab::Suggestions)
            };
            if hidden(self.tab) {
                self.tab = Tab::Overview;
            }
            let tab_before = self.tab;
            ui.horizontal(|ui| {
                for (t, name) in [
                    (Tab::Overview, "Overview"),
                    (Tab::Characters, "Characters"),
                    (Tab::Words, "Words"),
                    (Tab::Sections, "Sections"),
                    (Tab::Suggestions, "Suggested words"),
                    (Tab::Notes, "Notes & method"),
                ] {
                    if hidden(t) {
                        continue;
                    }
                    ui.selectable_value(&mut self.tab, t, RichText::new(name).size(15.0));
                }
            });
            // Marking words or importing lists changes how the book is
            // segmented; it's redone on returning to the Overview, not per click.
            if self.tab == Tab::Overview
                && tab_before != Tab::Overview
                && self.known_resegment_due()
            {
                self.rerun(&ctx, false);
            }
            ui.separator();
            match self.tab {
                Tab::Overview => self.overview(ui),
                Tab::Characters => self.char_table(ui),
                Tab::Words => self.word_table(ui),
                Tab::Sections => self.sections(ui),
                Tab::Suggestions => self.suggestions_tab(ui, &ctx),
                Tab::Notes => self.notes(ui),
            }
        });

        if self.anki.window {
            self.anki_window(&ctx);
        }
        if self.known.window {
            self.known_window(&ctx);
        }
        if self.forced_window {
            self.forced_window(&ctx);
        }
        if self.settings_window {
            self.settings_window(&ctx);
        }
        self.read_window(&ctx);
    }
}

impl App {
    /// The coloured mode badge: the book's language (and script, for Chinese).
    /// With no book open it chooses whose settings the windows show.
    fn mode_badge_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let variant = self
            .report
            .as_ref()
            .filter(|r| r.language == Language::Chinese)
            .map(|r| r.variant);
        let (text, fill) = mode_badge(self.language, variant);
        let resp = ui
            .add(
                egui::Button::new(RichText::new(text).color(Color32::WHITE).strong().size(15.0))
                    .fill(fill)
                    .corner_radius(egui::CornerRadius::same(6)),
            )
            .on_hover_text(
                "Language mode, detected from the book: Korean when Hangul outnumbers Chinese characters, Japanese when kana are at least a tenth as many as kanji. \
                 Anki decks and fields, word lists, marked words and the user dictionary are kept separately for each language. Click for details.",
            );
        let fixed = self.prep.is_some() || self.job.is_some();
        let mut pick = None;
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(260.0);
            ui.label(if fixed {
                "Set by the open book."
            } else {
                "No book open: choose which language's settings to show."
            });
            for l in Language::ALL {
                if ui
                    .add_enabled(
                        !fixed || l == self.language,
                        egui::RadioButton::new(self.language == l, language_label(l)),
                    )
                    .clicked()
                {
                    pick = Some(l);
                }
            }
            ui.separator();
            ui.label(
                RichText::new(format!(
                    "Saved per language: {} and {}.",
                    anki::Settings::path_for(self.language).display(),
                    known::Settings::path_for(self.language).display()
                ))
                .weak()
                .small(),
            );
        });
        if let Some(l) = pick {
            self.set_language(l, ctx);
        }
    }

    fn known_window(&mut self, ctx: &egui::Context) {
        let mut open = self.known.window;
        let mut changed = false;
        let lang = self.language;
        let title = match lang {
            Language::Chinese => "Known words: HSK, TOCFL & word lists (中文)",
            Language::Korean => "Known words: levels & word lists (한국어)",
            Language::Japanese => "Known words: JLPT & word lists (日本語)",
        };
        egui::Window::new(title)
            .id(egui::Id::unique("known-window"))
            .open(&mut open)
            .default_width(560.0)
            .show(ctx, |ui| {
                let k = &mut self.known;
                let bands = known::bands(lang);
                match lang {
                    Language::Chinese => {
                        ui.strong("HSK 3.0 vocabulary");
                        ui.label(
                            RichText::new(
                                "《HSK 考试大纲》, released November 2025 (exams from July 2026). Tick each level whose new words you know; levels 7–9 share one list.",
                            )
                            .weak(),
                        );
                    }
                    Language::Korean => {
                        ui.strong("Vocabulary by level (1–6급, as in TOPIK)");
                        ui.label(
                            RichText::new(
                                "국립국어원's 국제 통용 한국어 표준 교육과정 list (2017, revised 2020). TOPIK publishes no word list; this is the one TOPIK vocabulary lists follow. Tick each level whose new words you know. A 하다/되다 verb counts with its noun once 2급 is ticked.",
                            )
                            .weak(),
                        );
                    }
                    Language::Japanese => {
                        ui.strong("JLPT vocabulary (N5–N1)");
                        ui.label(
                            RichText::new(
                                "Jonathan Waller's lists, as used by jisho.org. The JLPT has published no word list since 2010, so these are an informed estimate. Tick each level whose new words you know. A long word that isn't on the list (冒険者) counts as known when its parts (冒険, 者) are.",
                            )
                            .weak(),
                        );
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    let mut cum = 0;
                    for &b in bands {
                        let n = known::band_count(lang, b);
                        cum += n;
                        let mut v = k.settings.band_on(b);
                        if ui
                            .checkbox(&mut v, format!("{} ({})", known::band_label(lang, b), fmt_int(n as u64)))
                            .on_hover_text(format!(
                                "{} words introduced at this level; {} up to and including it",
                                fmt_int(n as u64),
                                fmt_int(cum as u64)
                            ))
                            .changed()
                        {
                            k.settings.set_band(b, v);
                            changed = true;
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("All up to:").weak());
                    for (i, &b) in bands.iter().enumerate() {
                        let label = known::band_label(lang, b);
                        let short = label.trim_start_matches("HSK ");
                        let first = known::band_label(lang, bands[0]);
                        if ui.small_button(short).on_hover_text(format!("Tick {first} to {label} only")).clicked() {
                            // Bands are listed easiest first (JLPT counts down from N5).
                            k.settings.levels = bands[..=i].to_vec();
                            k.settings.levels.sort_unstable();
                            changed = true;
                        }
                    }
                    if ui.small_button("None").clicked() && !k.settings.levels.is_empty() {
                        k.settings.levels.clear();
                        changed = true;
                    }
                });
                if lang == Language::Chinese {
                    ui.add_space(6.0);
                    ui.strong("TOCFL vocabulary (華語八千詞表)");
                    ui.label(
                        RichText::new(
                            "SC-TOP's official list for the TOCFL (2024 edition, Traditional characters): Novice 1–2, then Level 1–2 (band A), Level 3–4 (band B) and Level 5 (band C). Level 6 has no list of its own; it is tested on Level 5's. \
                             Tick each level whose new words you know, alongside or instead of HSK. Variants count as listed (你/妳, 台灣/臺灣, 車/車子); with Match Traditional ↔ Simplified on, they match Simplified books too.",
                        )
                        .weak(),
                    );
                    ui.horizontal_wrapped(|ui| {
                        let mut cum = 0;
                        for &b in &known::TOCFL_LEVELS {
                            let n = known::tocfl_count(b);
                            cum += n;
                            let mut v = k.settings.tocfl_on(b);
                            if ui
                                .checkbox(&mut v, format!("{} ({})", known::tocfl_level_name(b), fmt_int(n as u64)))
                                .on_hover_text(format!(
                                    "Band {}. {} words introduced at this level; {} up to and including it",
                                    known::tocfl_band(b),
                                    fmt_int(n as u64),
                                    fmt_int(cum as u64)
                                ))
                                .changed()
                            {
                                k.settings.set_tocfl(b, v);
                                changed = true;
                            }
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("All up to:").weak());
                        for (i, &b) in known::TOCFL_LEVELS.iter().enumerate() {
                            let label = known::tocfl_level_name(b);
                            if ui
                                .small_button(label)
                                .on_hover_text(format!("Tick Novice 1 to {label} only"))
                                .clicked()
                            {
                                k.settings.tocfl = known::TOCFL_LEVELS[..=i].to_vec();
                                changed = true;
                            }
                        }
                        if ui.small_button("None").clicked() && !k.settings.tocfl.is_empty() {
                            k.settings.tocfl.clear();
                            changed = true;
                        }
                    });
                }
                if lang == Language::Japanese {
                    ui.add_space(4.0);
                    if ui
                        .checkbox(
                            &mut k.settings.kaishi,
                            format!(
                                "{} deck ({} words)",
                                known::KAISHI_NAME,
                                fmt_int(known::kaishi_count() as u64)
                            ),
                        )
                        .on_hover_text(
                            "Every word of the Kaishi 1.5k Anki deck (donkuri/Kaishi v2.4.3, 1,500 cards) counts as known, for when you studied it outside the Anki collection hanstat reads. \
                             If the deck is in that collection, tick it in the Anki window instead, so that only the cards you have learned count.",
                        )
                        .changed()
                    {
                        changed = true;
                    }
                }
                ui.separator();

                ui.horizontal(|ui| {
                    ui.strong("Word lists");
                    if ui.button("📂 Import TXT…").clicked()
                        && let Some(paths) = rfd::FileDialog::new()
                            .add_filter("Word lists", &["txt", "csv", "tsv"])
                            .add_filter("All files", &["*"])
                            .pick_files()
                    {
                        let mut errors = Vec::new();
                        for p in paths {
                            match known::read_word_list_in(&p, lang) {
                                Ok(l) => {
                                    k.settings.add_list(l);
                                    changed = true;
                                }
                                Err(e) => errors.push(e),
                            }
                        }
                        k.error = (!errors.is_empty()).then(|| errors.join("\n"));
                    }
                });
                let mut remove = None;
                if k.settings.lists.is_empty() {
                    ui.label(RichText::new("No lists imported.").weak());
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("known-lists")
                        .max_height(180.0)
                        .show(ui, |ui| {
                            egui::Grid::new("known-lists-grid").striped(true).spacing([16.0, 4.0]).show(ui, |ui| {
                                for (i, l) in k.settings.lists.iter().enumerate() {
                                    ui.label(&l.name).on_hover_text(l.path.display().to_string());
                                    ui.label(format!("{} words", fmt_int(l.words.len() as u64)));
                                    if ui.small_button("✖").on_hover_text("Remove this list").clicked() {
                                        remove = Some(i);
                                    }
                                    ui.end_row();
                                }
                            });
                        });
                }
                if let Some(i) = remove {
                    k.settings.lists.remove(i);
                    changed = true;
                }
                ui.label(
                    RichText::new(match lang {
                        Language::Chinese => "One word per line. The first item on a line that contains Chinese is used, so exports with pinyin or definitions after a tab, comma or space work; # starts a comment. \
                         Lists are copied in: import a file again to update it.",
                        Language::Korean => "One word per line, in dictionary form (먹다, 공부하다, 사과), since the book's words are counted that way. The first item on a line that contains Hangul is used, so exports with definitions after a tab, comma or space work; # starts a comment. \
                         Lists are copied in: import a file again to update it.",
                        Language::Japanese => "One word per line, in any spelling or form: entries are read with Sudachi like the book, so 分かる, わかる and わかった all mark 分かる, and each word of an entry such as 勉強する counts. The first item on a line that contains kana or kanji is used, so exports with readings or definitions after a tab, comma or space work; furigana in [brackets] is dropped; # starts a comment. \
                         Lists are copied in: import a file again to update it.",
                    })
                    .weak()
                    .small(),
                );

                ui.separator();
                ui.horizontal(|ui| {
                    ui.strong(format!("Marked in the Words tab ({})", fmt_int(k.settings.marked.len() as u64)));
                    if !k.settings.marked.is_empty()
                        && ui
                            .small_button("Clear all")
                            .on_hover_text("Unmark every word you marked with +")
                            .clicked()
                    {
                        k.settings.marked.clear();
                        changed = true;
                    }
                });
                if k.settings.marked.is_empty() {
                    ui.label(RichText::new("Click + next to a word in the Words tab to mark it known.").weak());
                } else {
                    let mut unmark = None;
                    egui::ScrollArea::vertical()
                        .id_salt("known-marked")
                        .max_height(140.0)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                // Newest first.
                                for w in k.settings.marked.iter().rev() {
                                    if ui
                                        .small_button(format!("{w} ✖"))
                                        .on_hover_text("Unmark")
                                        .clicked()
                                    {
                                        unmark = Some(w.clone());
                                    }
                                }
                            });
                        });
                    if let Some(w) = unmark {
                        changed |= k.settings.set_marked(&w, false);
                    }
                }

                ui.separator();
                ui.strong(format!("Books read ({})", fmt_int(k.settings.read.len() as u64)));
                if k.settings.read.is_empty() {
                    ui.label(
                        RichText::new("Mark a book read with ✔ in the Library to count its most frequent words as known.")
                            .weak(),
                    );
                } else {
                    let mut unread = None;
                    egui::ScrollArea::vertical()
                        .id_salt("known-read")
                        .max_height(100.0)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                for b in k.settings.read.iter().rev() {
                                    if ui
                                        .small_button(format!("{} ✖", b.title))
                                        .on_hover_text(format!(
                                            "{} words ({}% of those read {}+ times), marked read {}. Click to unmark it and take them back.",
                                            fmt_int(b.words.len() as u64),
                                            b.percent,
                                            b.min_count,
                                            history::date(b.read)
                                        ))
                                        .clicked()
                                    {
                                        unread = Some(b.id.clone());
                                    }
                                }
                            });
                        });
                    if let Some(id) = unread {
                        changed |= k.settings.unset_read(&id);
                    }
                }
                if let Some(e) = &k.error {
                    ui.colored_label(palette::error(), format!("⚠ {e}"));
                }

                ui.separator();
                let anki_on = self.anki.settings.enabled && self.anki.knowledge.is_some();
                match k.merged.as_ref().filter(|_| !k.settings.is_empty()) {
                    Some(m) if lang == Language::Korean => {
                        let l = &m.listed;
                        ui.label(if anki_on {
                            format!(
                                "{} distinct words marked known; {} of them are not young/mature in Anki.",
                                fmt_int(l.words),
                                fmt_int(l.new_words)
                            )
                        } else {
                            format!(
                                "{} distinct words marked known. Anki is off, so only these count.",
                                fmt_int(l.words)
                            )
                        });
                    }
                    Some(m) => {
                        let l = &m.listed;
                        let chars = if lang == Language::Japanese { "kanji" } else { "characters" };
                        ui.label(if anki_on {
                            format!(
                                "{} distinct words marked known; {} of them are not young/mature in Anki, adding {} {chars}.",
                                fmt_int(l.words),
                                fmt_int(l.new_words),
                                fmt_int(l.new_chars)
                            )
                        } else {
                            format!(
                                "{} distinct words marked known ({} {chars}). Anki is off, so only these count.",
                                fmt_int(l.words),
                                fmt_int(l.new_chars)
                            )
                        });
                    }
                    None => {
                        ui.label("Nothing marked known yet.");
                    }
                }
                ui.label(
                    RichText::new(match lang {
                        Language::Chinese => format!(
                            "These words count as known (\"Word list\" in the Characters and Words tabs) alongside young and mature Anki cards, and a character is known if it is in a known word. \
                             Traditional ↔ Simplified matching follows the Anki window's option (now {}). Saved to {}.",
                            if self.anki.settings.match_across_scripts { "on" } else { "off" },
                            known::Settings::path_for(lang).display()
                        ),
                        Language::Korean => format!(
                            "These words count as known (\"Word list\" in the Words tab) alongside young and mature Anki cards. \
                             Korean lists and marked words are kept apart from the Chinese ones. Saved to {}.",
                            known::Settings::path_for(lang).display()
                        ),
                        Language::Japanese => format!(
                            "These words count as known (\"Word list\" in the Characters and Words tabs) alongside young and mature Anki cards, and a kanji is known if it is in a known word. \
                             Japanese lists and marked words are kept apart from the Chinese and Korean ones. Saved to {}.",
                            known::Settings::path_for(lang).display()
                        ),
                    })
                    .weak()
                    .small(),
                );
            });
        self.known.window = open;
        if changed {
            self.known_changed();
        }
    }

    /// Levels, lists or marked words changed: save them and recompute coverage.
    fn known_changed(&mut self) {
        if let Err(e) = self.known.settings.save() {
            self.known.error = Some(e);
        }
        self.refresh_anki_coverage();
    }

    /// True if the open Chinese book was segmented with other imported word
    /// lists than the current ones.
    fn known_resegment_due(&self) -> bool {
        self.job.is_none()
            && self.prep.as_ref().is_some_and(|p| {
                p.language == Language::Chinese
                    && p.language == self.language
                    && p.known_words != self.known.settings.segmenter_words()
            })
    }
}

/// Grow the window's inner height to `want` points, without going past the
/// bottom of the screen (minus the taskbar). Never shrinks; does nothing when
/// maximised or full-screen.
fn grow_window(ctx: &egui::Context, want: f32) {
    /// Windows' taskbar at 100–200% scaling is 40–48 points tall. On macOS the
    /// menu bar (24–37 points with a notch) and the default Dock (about 70)
    /// are both taken from the screen height.
    const TASKBAR: f32 = if cfg!(target_os = "macos") {
        110.0
    } else {
        48.0
    };
    let (inner, outer, monitor, maxed) = ctx.input(|i| {
        let v = i.viewport();
        (
            v.inner_rect,
            v.outer_rect,
            v.monitor_size,
            v.maximized == Some(true) || v.fullscreen == Some(true),
        )
    });
    let (Some(inner), Some(outer), Some(monitor)) = (inner, outer, monitor) else {
        return;
    };
    if maxed {
        return;
    }
    let chrome = (outer.height() - inner.height()).max(0.0);
    let usable = monitor.y - TASKBAR;
    let target = want.min(usable - chrome);
    if target <= inner.height() + 1.0 {
        return;
    }
    // Move the window up if growing would push it below the usable area. Only
    // when its position looks like it is on the primary monitor (origin 0,0);
    // otherwise just grow.
    let bottom = outer.top() + chrome + target;
    if outer.top() >= 0.0 && outer.top() < monitor.y && bottom > usable {
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
            outer.left(),
            (usable - chrome - target).max(0.0),
        )));
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
        inner.width(),
        target,
    )));
}

fn anki_filter_combo(ui: &mut egui::Ui, f: &mut AnkiFilter, available: bool) {
    if !available {
        return;
    }
    ui.label("Known:");
    egui::ComboBox::from_id_salt("anki-filter")
        .selected_text(f.label())
        .show_ui(ui, |ui| {
            for o in AnkiFilter::OPTIONS {
                ui.selectable_value(f, o, o.label());
            }
        });
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}
/// Open `path` in the program the system uses for it (a text editor for a log).
fn open_with_system(path: &Path) -> Result<(), String> {
    let mut cmd = if cfg!(windows) {
        // Explorer opens a file with its default program (no cmd quoting pitfalls).
        let mut c = std::process::Command::new("explorer");
        c.arg(path);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(path);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path);
        c
    };
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("Cannot open {}: {e}", path.display()))
}

/// A Library book matches the search by its title or its file name.
fn book_matches(q: &search::Query, title: &str, path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy());
    q.matches(std::iter::once(title).chain(name.as_deref()))
}

fn file_stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn save(name: &str, ext: &str, content: String) -> Option<String> {
    let p = rfd::FileDialog::new()
        .set_file_name(name)
        .add_filter(ext.to_uppercase(), &[ext])
        .save_file()?;
    Some(match std::fs::write(&p, content) {
        Ok(()) => format!("Saved {}", file_name(&p)),
        Err(e) => format!("Save failed: {e}"),
    })
}

fn fmt_int(n: u64) -> String {
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

/// Counts by Anki status (the Anki window's summary of the selection).
fn status_grid(ui: &mut egui::Ui, id: &str, rows: &[(&str, &ByStatus)]) {
    let cols = [Status::Mature, Status::Young, Status::Learning, Status::New];
    egui::Grid::new(id)
        .striped(true)
        .num_columns(cols.len() + 2)
        .spacing([24.0, 4.0])
        .show(ui, |ui| {
            ui.label("");
            ui.strong("Known").on_hover_text("Young + mature");
            for s in cols {
                ui.label(status_text(s).strong());
            }
            ui.end_row();
            for (label, b) in rows {
                ui.label(*label);
                ui.strong(fmt_int(b.known()));
                for s in cols {
                    ui.label(fmt_int(b.get(s)));
                }
                ui.end_row();
            }
        });
}

fn big_stat(ui: &mut egui::Ui, title: &str, value: u64, sub: &str) {
    stat_tile(ui, title, RichText::new(fmt_int(value)), sub);
}

fn stat_tile(ui: &mut egui::Ui, title: &str, value: RichText, sub: &str) -> egui::Response {
    stat_tile_info(ui, title, value, sub, None)
}

/// A tile with an ⓘ beside its title that shows `info` when clicked.
fn stat_tile_info(
    ui: &mut egui::Ui,
    title: &str,
    value: RichText,
    sub: &str,
    info: Option<&dyn Fn(&mut egui::Ui)>,
) -> egui::Response {
    egui::Frame::group(ui.style())
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_min_width(230.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).weak());
                    if let Some(f) = info {
                        info_button(ui, |ui| f(ui));
                    }
                });
                ui.label(value.size(30.0).strong());
                ui.label(sub);
            });
        })
        .response
}

/// The Overview's explanation of the writing-style rating.
const STYLE_HOVER: &str = "How hard the prose is to read when you know its words: sentence structure, written and classical register, rhetoric and description, what you must infer. Rated by a local AI model (Qwen3.5-4B) from four samples of 3,000 characters. The percentage compares it with the rated books in your Library: 50% is their median, and 9 in 10 of them fall between 5% and 95%. The word after it (plain, typical, literary, very literary) is the model's own judgment, the same for everyone. It counts toward the difficulty with the weight set in Settings (⚙), where it can be turned off.";

impl App {
    /// The writing-style scale fitted to the rated Chinese books of the
    /// Library ([`style_scale`]).
    fn style_scale(&self) -> StyleScale {
        let rated = self.style.store.current();
        StyleScale::fit(
            self.history
                .entries
                .iter()
                .filter(|e| e.stats.language == Language::Chinese)
                .filter_map(|e| rated.get(e.id.as_str()).copied()),
        )
    }
}

fn difficulty_color(d: Difficulty) -> Color32 {
    let (dark, light) = match d {
        Difficulty::Easy => ((90, 190, 110), (28, 125, 55)),
        Difficulty::Moderate => ((170, 200, 80), (95, 115, 0)),
        Difficulty::Challenging => ((220, 170, 60), (160, 95, 0)),
        Difficulty::Hard => ((230, 120, 60), (190, 70, 10)),
        Difficulty::VeryHard => ((220, 80, 80), (195, 40, 40)),
    };
    palette::pick(
        Color32::from_rgb(dark.0, dark.1, dark.2),
        Color32::from_rgb(light.0, light.1, light.2),
    )
}

impl App {
    fn overview(&mut self, ui: &mut egui::Ui) {
        let rec = self
            .suggestions
            .as_ref()
            .map(|v| v.iter().filter(|c| c.recommended).count())
            .unwrap_or(0);
        if rec > 0 {
            let msg = format!(
                "💡 {rec} likely names/terms are being split by the segmenter, which inflates unique words. Review them in Suggested words."
            );
            if ui.link(RichText::new(msg).color(palette::warn())).clicked() {
                self.tab = Tab::Suggestions;
            }
            ui.add_space(4.0);
        }
        let r = self.report.as_ref().unwrap();
        // A Korean book has word statistics only; a Japanese one has words
        // laid out like Korean, plus kanji.
        let chinese = r.language == Language::Chinese;
        let japanese = r.language == Language::Japanese;
        let has_chars = chinese || japanese;
        let lang_name = r.language.name();
        // Writing style, while ratings are on: (rating, hover). Shown in the
        // difficulty tile, which it counts toward; a tile of its own when
        // nothing is marked known.
        let style_info: Option<(String, String)> = (chinese && self.style_on()).then(|| {
            let id = self.report_id.as_deref().unwrap_or("");
            let scale = self.style_scale();
            match self.style.store.get(id) {
                Some(s) => (
                    format!("{} · {}", scale.pct(s.score), s.label()),
                    format!(
                        "Writing style {} against the {} rated books of your Library (50% is their median). \
                         The model's own rating: {:.2} on its scale of 1–9 (typical {:.2}); \
                         samples at 30, 45, 60 and 75% of the text: {}",
                        scale.pct(s.score),
                        scale.books,
                        s.score,
                        style::TYPICAL,
                        s.samples.iter().map(|x| format!("{x:.2}")).collect::<Vec<_>>().join(", ")
                    ),
                ),
                None => match self.style.skipped.get(id) {
                    Some(why) => ("not rated".into(), why.clone()),
                    None => (
                        "being rated…".into(),
                        "The local model rates this book in the background.".into(),
                    ),
                },
            }
        });
        let style_off = self
            .report_id
            .as_deref()
            .map_or(0.0, |id| self.style_offset(id));
        let skip_hapax = self.settings.skip_hapax;
        let out = egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if let Some(t) = &r.title {
                ui.heading(t);
            }
            let (included, sections) = (self.included.iter().filter(|x| **x).count(), self.included.len());
            ui.label(if chinese {
                format!(
                    "{} · {:?} Chinese (traditional-only chars: {}, simplified-only: {}) · {} of {} sections included",
                    r.format,
                    r.variant,
                    fmt_int(r.trad_marks),
                    fmt_int(r.simp_marks),
                    included,
                    sections
                )
            } else if japanese {
                format!(
                    "{} · Japanese · words are counted in dictionary form with spellings merged (わかった, 分かる → 分かる) · {} of {} sections included",
                    r.format, included, sections
                )
            } else {
                format!(
                    "{} · Korean · words are counted in dictionary form (먹었어요 → 먹다) · {} of {} sections included",
                    r.format, included, sections
                )
            });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if chinese {
                    big_stat(ui, "Chinese characters (total)", r.han_total, &format!("{} occur only once", fmt_int(r.han_hapax as u64)));
                    big_stat(ui, "Unique characters", r.han_unique as u64, &format!("{} needed for 99% coverage", r.char_coverage.iter().find(|c| c.percent == 99.0).map(|c| c.items).unwrap_or(0)));
                    big_stat(ui, "Chinese words (total)", r.words_total, &format!("segmenter: {}", r.segmenter.label()));
                } else {
                    if japanese {
                        big_stat(ui, "Kanji (total)", r.han_total, &format!("{} occur only once", fmt_int(r.han_hapax as u64)));
                        big_stat(ui, "Unique kanji", r.han_unique as u64, &format!("{} needed for 99% coverage", r.char_coverage.iter().find(|c| c.percent == 99.0).map(|c| c.items).unwrap_or(0)));
                    }
                    big_stat(ui, &format!("{lang_name} words (total)"), r.words_total, &format!("analyser: {}", r.segmenter.label()));
                }
                big_stat(
                    ui,
                    "Unique words",
                    r.words_unique as u64,
                    &format!(
                        "{} occur only once{}",
                        fmt_int(r.words_hapax as u64),
                        if skip_hapax { " · left out (⚙)" } else { "" }
                    ),
                );
                if let Some((value, hover)) = &style_info
                    && r.anki.is_none()
                {
                    // On a row of its own: the tiles don't wrap.
                    ui.end_row();
                    let explain = |ui: &mut egui::Ui| {
                        ui.label(hover);
                        ui.add_space(4.0);
                        ui.label(RichText::new(STYLE_HOVER).weak());
                    };
                    stat_tile_info(
                        ui,
                        "Writing style",
                        RichText::new(value),
                        "against your Library: 50% is its median",
                        Some(&explain),
                    )
                    .on_hover_ui(explain);
                }
                // Korean only: a Japanese book already has four tiles, and the
                // coverage table below gives the same number.
                if !has_chars {
                    big_stat(ui, "Words for 98% coverage", r.word_coverage.iter().find(|c| c.percent == 98.0).map_or(0, |c| c.items as u64), "most frequent words, see Coverage below");
                }
            });
            ui.add_space(10.0);

            if let Some(c) = &r.anki {
                let lists = &c.listed.source;
                ui.heading(match (c.from_anki, lists.is_empty()) {
                    (true, true) => "Known from Anki".to_owned(),
                    (true, false) => format!("Known from Anki + {lists}"),
                    (false, _) => format!("Known from {lists}"),
                });
                ui.horizontal_wrapped(|ui| {
                    let tiles: &[(&str, &anki::KnownCount)] = if chinese {
                        &[
                            ("Unique characters you know", &c.unique_chars),
                            ("Unique words you know", &c.unique_words),
                        ]
                    } else if japanese {
                        &[
                            ("Unique kanji you know", &c.unique_chars),
                            ("Unique words you know", &c.unique_words),
                        ]
                    } else {
                        &[("Unique words you know", &c.unique_words)]
                    };
                    for (title, k) in tiles {
                        big_stat(
                            ui,
                            title,
                            k.known,
                            &format!(
                                "{:.1}% of {} · {} not known",
                                k.percent(),
                                fmt_int(k.total),
                                fmt_int(k.not_known())
                            ),
                        );
                    }
                    let rw = &c.running_words;
                    let unknown = rw.not_known();
                    let hapax_note = if c.hapax_left_out > 0 {
                        format!(
                            "{} words that occur only once are left out of these numbers, known or not (Settings, ⚙).\n\n",
                            fmt_int(c.hapax_left_out)
                        )
                    } else {
                        String::new()
                    };
                    let running_text = hapax_note + &if chinese {
                        format!(
                            "Every occurrence counts, so frequent words weigh more. {} of {} word occurrences are known; {} occurrences of names (your user dictionary and the recommended Suggested words) are left out. \
                             Of the running characters, {:.1}% are known.",
                            fmt_int(rw.known),
                            fmt_int(rw.total),
                            fmt_int(c.name_tokens),
                            c.running_chars.percent()
                        )
                    } else if japanese {
                        format!(
                            "Every occurrence counts, so frequent words weigh more. {} of {} word occurrences are known; {} occurrences of names (your user dictionary) are left out. \
                             Particles and auxiliaries are grammar, not words. Of the running kanji, {:.1}% are known.",
                            fmt_int(rw.known),
                            fmt_int(rw.total),
                            fmt_int(c.name_tokens),
                            c.running_chars.percent()
                        )
                    } else {
                        format!(
                            "Every occurrence counts, so frequent words weigh more. {} of {} word occurrences are known; {} occurrences of names (your user dictionary) are left out.",
                            fmt_int(rw.known),
                            fmt_int(rw.total),
                            fmt_int(c.name_tokens)
                        )
                    };
                    let explain_running = |ui: &mut egui::Ui| {
                        ui.label(&running_text);
                    };
                    stat_tile_info(
                        ui,
                        "Running words you know",
                        RichText::new(format!("{:.1}%", rw.percent())),
                        &if unknown > 0 {
                            format!("1 unknown every {:.1} words", rw.total as f64 / unknown as f64)
                        } else {
                            "no unknown words".to_owned()
                        },
                        Some(&explain_running),
                    )
                    .on_hover_ui(explain_running);
                    // Writing style counts as a share of words known (Settings).
                    let counted = rw.percent() - style_off;
                    let d = Difficulty::from_percent(counted);
                    let styled = style_off.abs() >= 0.05;
                    let mut sub = d.description().to_owned();
                    if let Some((value, _)) = &style_info {
                        sub.push_str(&format!("\nWriting style {value}"));
                        if styled {
                            sub.push_str(&format!(" · counts as {counted:.1}% known"));
                        }
                    }
                    let explain = |ui: &mut egui::Ui| {
                        if let Some((_, hover)) = &style_info {
                            ui.label(hover);
                            if styled {
                                ui.label(format!(
                                    "{:.1}% of running words known, {} {:.1} points for the writing style (weight in Settings, ⚙): counts as {counted:.1}%.",
                                    rw.percent(),
                                    if style_off > 0.0 { "less" } else { "plus" },
                                    style_off.abs()
                                ));
                            }
                            ui.label(RichText::new(STYLE_HOVER).weak().small());
                            ui.separator();
                        }
                        ui.label("From the share of running words you know:");
                        egui::Grid::new("difficulty-bands").spacing([16.0, 2.0]).show(ui, |ui| {
                            let mut upper = None;
                            for (min, band) in Difficulty::BANDS
                                .iter()
                                .copied()
                                .chain([(0.0, Difficulty::VeryHard)])
                            {
                                let text = RichText::new(band.label()).color(difficulty_color(band));
                                ui.label(if band == d { text.strong() } else { text });
                                ui.label(match upper {
                                    None => format!("{min:.0}% or more"),
                                    Some(u) if min == 0.0 => format!("under {u:.0}%"),
                                    Some(u) => format!("{min:.0} – {u:.0}%"),
                                });
                                ui.end_row();
                                upper = Some(min);
                            }
                        });
                        ui.label(
                            RichText::new(if chinese {
                                "Thresholds from L2 reading research: 98% coverage for comfortable reading of fiction (Hu & Nation 2000), 95% as the minimum for adequate comprehension (Laufer 1989). \
                                 Words you know but have no Anki card for (and aren't in a ticked HSK or TOCFL level or word list) count as unknown, so the book may be easier than this."
                            } else if japanese {
                                "Thresholds from L2 reading research: 98% coverage for comfortable reading of fiction (Hu & Nation 2000), 95% as the minimum for adequate comprehension (Laufer 1989). \
                                 Words you know but have no Anki card for (and aren't in a ticked JLPT level or word list) count as unknown, so the book may be easier than this."
                            } else {
                                "Thresholds from L2 reading research: 98% coverage for comfortable reading of fiction (Hu & Nation 2000), 95% as the minimum for adequate comprehension (Laufer 1989). \
                                 Words you know but have no Anki card for (and aren't in a ticked level or word list) count as unknown, so the book may be easier than this."
                            })
                            .weak()
                            .small(),
                        );
                    };
                    stat_tile_info(
                        ui,
                        "Difficulty for you",
                        RichText::new(d.label()).color(difficulty_color(d)),
                        &sub,
                        Some(&explain),
                    )
                    .on_hover_ui(explain);
                });
                let anki_part = if has_chars {
                    format!(
                        "on a young or mature card in your selected decks ({} words, {} {})",
                        fmt_int(c.anki.words.known()),
                        fmt_int(c.anki.chars.known()),
                        if japanese { "kanji" } else { "characters" },
                    )
                } else {
                    format!(
                        "on a young or mature card in your selected decks ({} words)",
                        fmt_int(c.anki.words.known()),
                    )
                };
                let list_part = format!(
                    "in {} ({} words{})",
                    lists,
                    fmt_int(c.listed.words),
                    if c.from_anki {
                        format!(", {} not already known from Anki", fmt_int(c.listed.new_words))
                    } else {
                        String::new()
                    }
                );
                let known_def = match (c.from_anki, lists.is_empty()) {
                    (true, true) => anki_part,
                    (true, false) => format!("{anki_part}, or {list_part}"),
                    (false, _) => list_part,
                };
                ui.label(
                    RichText::new(if chinese {
                        format!(
                            "Known = {known_def}. A character is known if it appears in a known word. Click ⓘ on the last two tiles for details."
                        )
                    } else if japanese {
                        format!(
                            "Known = {known_def}. A kanji is known if it appears in a known word, and a long word (冒険者) if all its parts (冒険, 者) are known. Click ⓘ on the last two tiles for details."
                        )
                    } else {
                        format!("Known = {known_def}. Click ⓘ on the last two tiles for details.")
                    })
                    .weak(),
                );
                ui.add_space(10.0);
            }

            if !chinese {
                // Korean: words only. Japanese: words, plus kanji.
                ui.columns(2, |cols| {
                    let ui = &mut cols[0];
                    ui.heading("Composition");
                    egui::Grid::new("comp").striped(true).num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                        let mut rows: Vec<(String, u64)> = vec![
                            (format!("{lang_name} words (total)"), r.words_total),
                            (format!("Non-{lang_name} words (total)"), r.foreign_words_total),
                            (format!("Non-{lang_name} words (unique)"), r.foreign_words_unique as u64),
                            ("Numbers".into(), r.numbers),
                            ("Private-use characters (unreadable)".into(), r.private_use),
                            ("Decode errors (U+FFFD)".into(), r.replacement),
                        ];
                        if japanese {
                            rows.insert(1, ("Kanji (total)".into(), r.han_total));
                            rows.insert(2, ("Kana (total)".into(), r.kana_total));
                        }
                        for (k, v) in rows {
                            ui.label(k);
                            ui.label(fmt_int(v));
                            ui.end_row();
                        }
                    });
                    ui.add_space(10.0);
                    ui.heading("Structure");
                    egui::Grid::new("struct").striped(true).num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                        ui.label("Paragraphs");
                        ui.label(fmt_int(r.paragraphs));
                        ui.end_row();
                        ui.label("Sentences");
                        ui.label(fmt_int(r.sentences));
                        ui.end_row();
                        ui.label("Avg. sentence length");
                        ui.label(format!("{:.1} words", r.avg_sentence_words.unwrap_or(0.0)));
                        ui.end_row();
                        ui.label("Avg. paragraph length");
                        ui.label(format!("{:.1} words", r.avg_paragraph_words.unwrap_or(0.0)));
                        ui.end_row();
                        ui.label("Reading time");
                        ui.horizontal(|ui| {
                            // Japanese reading speed is measured in characters (文字/分), Korean in words.
                            let (speed, default) = if japanese {
                                (&mut self.ja_cpm, self.settings.ja_cpm)
                            } else {
                                (&mut self.wpm, self.settings.wpm)
                            };
                            let v = speed.unwrap_or(default) as f64;
                            let mins = if japanese {
                                (r.han_total + r.kana_total) as f64 / v
                            } else {
                                r.words_total as f64 / v
                            };
                            ui.label(format!("{}h {:02}m at", (mins / 60.0) as u64, (mins % 60.0) as u64));
                            if japanese {
                                speed_input(ui, speed, default, 50.0..=2000.0, 5.0, " chars/min");
                            } else {
                                speed_input(ui, speed, default, 20.0..=1000.0, 2.0, " words/min");
                            }
                        })
                        .response
                        .on_hover_text(if japanese {
                            "Characters are kanji and kana. Punctuation, digits and Latin letters are not counted."
                        } else {
                            "Words as counted here: dictionary forms of content words, without particles and endings."
                        });
                        ui.end_row();
                    });

                    let ui = &mut cols[1];
                    ui.heading("Coverage");
                    if japanese {
                        ui.label(RichText::new("How many of the most frequent words (kanji) you must know to recognise X% of the running words (kanji).").weak());
                        egui::Grid::new("cov").striped(true).num_columns(3).spacing([32.0, 4.0]).show(ui, |ui| {
                            ui.strong("Coverage");
                            ui.strong("Words");
                            ui.strong("Kanji");
                            ui.end_row();
                            for (i, w) in r.word_coverage.iter().enumerate() {
                                ui.label(format!("{}%", w.percent));
                                ui.label(fmt_int(w.items as u64));
                                ui.label(r.char_coverage.get(i).map_or("–".into(), |c| fmt_int(c.items as u64)));
                                ui.end_row();
                            }
                        });
                    } else {
                        ui.label(RichText::new("How many of the most frequent words you must know to recognise X% of the running words.").weak());
                        egui::Grid::new("cov").striped(true).num_columns(2).spacing([32.0, 4.0]).show(ui, |ui| {
                            ui.strong("Coverage");
                            ui.strong("Words");
                            ui.end_row();
                            for w in &r.word_coverage {
                                ui.label(format!("{}%", w.percent));
                                ui.label(fmt_int(w.items as u64));
                                ui.end_row();
                            }
                        });
                    }
                });
                return;
            }
            ui.columns(2, |cols| {
                let ui = &mut cols[0];
                ui.heading("Composition");
                egui::Grid::new("comp").striped(true).num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                    let rows: [(&str, u64); 10] = [
                        ("All non-whitespace characters", r.nonspace_total),
                        ("  Chinese characters", r.han_total),
                        ("  Punctuation", r.punctuation),
                        ("  Other letters (Latin, kana…)", r.letters),
                        ("  Digits", r.digits),
                        ("  Symbols / emoji", r.symbols),
                        ("  Private-use (unreadable)", r.private_use),
                        ("  Decode errors (U+FFFD)", r.replacement),
                        ("Non-Chinese words (total)", r.foreign_words_total),
                        ("Numbers", r.numbers),
                    ];
                    for (k, v) in rows {
                        ui.label(k);
                        ui.label(fmt_int(v));
                        ui.end_row();
                    }
                    ui.label("Non-Chinese words (unique)");
                    ui.label(fmt_int(r.foreign_words_unique as u64));
                    ui.end_row();
                });
                ui.add_space(10.0);
                ui.heading("Structure");
                egui::Grid::new("struct").striped(true).num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                    ui.label("Paragraphs");
                    ui.label(fmt_int(r.paragraphs));
                    ui.end_row();
                    ui.label("Sentences");
                    ui.label(fmt_int(r.sentences));
                    ui.end_row();
                    ui.label("Avg. sentence length");
                    ui.label(format!("{:.1} characters", r.avg_sentence_han));
                    ui.end_row();
                    ui.label("Avg. paragraph length");
                    ui.label(format!("{:.1} characters", r.avg_paragraph_han));
                    ui.end_row();
                    ui.label("Avg. word length");
                    ui.label(format!("{:.2} characters", r.avg_word_len));
                    ui.end_row();
                    ui.label("Reading time");
                    ui.horizontal(|ui| {
                        let mins = r.han_total as f64 / self.cpm.unwrap_or(self.settings.cpm) as f64;
                        ui.label(format!("{}h {:02}m at", (mins / 60.0) as u64, (mins % 60.0) as u64));
                        speed_input(ui, &mut self.cpm, self.settings.cpm, 50.0..=2000.0, 5.0, " chars/min");
                    });
                    ui.end_row();
                });

                let ui = &mut cols[1];
                ui.heading("Coverage");
                ui.label(RichText::new("How many of the most frequent items you must know to recognise X% of the running text.").weak());
                egui::Grid::new("cov").striped(true).num_columns(3).spacing([32.0, 4.0]).show(ui, |ui| {
                    ui.strong("Coverage");
                    ui.strong("Characters");
                    ui.strong("Words");
                    ui.end_row();
                    for (c, w) in r.char_coverage.iter().zip(&r.word_coverage) {
                        ui.label(format!("{}%", c.percent));
                        ui.label(fmt_int(c.items as u64));
                        ui.label(fmt_int(w.items as u64));
                        ui.end_row();
                    }
                });
                ui.add_space(10.0);
                ui.heading("Word length (share of running words)");
                let total = r.words_total.max(1) as f32;
                for (i, n) in r.word_length_tokens.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(if i == 4 { "5+ chars".to_owned() } else { format!("{} char{}", i + 1, if i == 0 { " " } else { "s" }) });
                        let f = *n as f32 / total;
                        ui.add(egui::ProgressBar::new(f).desired_width(220.0).text(format!("{:.1}%", f * 100.0)));
                    });
                }
            });
        });
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        if std::mem::take(&mut self.fit_pending) {
            self.fit_until = now + 3.0;
            self.fit_requested = 0.0;
        }
        // Measure only on frames egui will actually show (not sizing passes).
        if self.auto_fit && now < self.fit_until && !ctx.will_discard() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            let overflow = out.content_size.y - out.inner_rect.height();
            let inner_h = ctx.input(|i| i.viewport().inner_rect.map(|r| r.height()));
            if overflow > 0.5
                && let Some(h) = inner_h
            {
                let want = h + overflow.ceil() + 4.0;
                // A resize already requested may not have landed yet.
                if want > self.fit_requested + 1.0 {
                    self.fit_requested = want;
                    grow_window(&ctx, want);
                }
            }
        }
    }

    /// The Characters table's rows (indexes into `char_freq`), filtered and
    /// sorted; made again only when the report, the filters or the sort change.
    fn char_rows(&mut self) -> Arc<Vec<usize>> {
        let key = RowsKey {
            generation: self.report_gen,
            filter: self.char_filter.clone(),
            known: self.anki_filter,
            sort: self.char_sort,
        };
        if let Some((k, rows)) = &self.char_rows
            && *k == key
        {
            return rows.clone();
        }
        let r = self.report.as_ref().unwrap();
        let status = r.anki.as_ref().map(|c| &c.char_status);
        // Folded counts list a character once, as the book writes it: 说
        // finds 說 there.
        let fold = |c: char| anki::fold_char(c, r.chars_folded);
        let f: Vec<char> = key
            .filter
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(fold)
            .collect();
        let mut rows: Vec<usize> = (0..r.char_freq.len())
            .filter(|&i| f.is_empty() || f.contains(&fold(r.char_freq[i].0)))
            .filter(|&i| status.is_none_or(|s| key.known.matches(s[i])))
            .collect();
        let (col, desc) = key.sort;
        // Ties stay in frequency order.
        rows.sort_by(|&a, &b| {
            let o = match col {
                CharCol::Rank => a.cmp(&b),
                CharCol::Char => r.char_freq[a].0.cmp(&r.char_freq[b].0),
                CharCol::Status => status.map_or(std::cmp::Ordering::Equal, |s| s[a].cmp(&s[b])),
            };
            (if desc { o.reverse() } else { o }).then(a.cmp(&b))
        });
        let rows = Arc::new(rows);
        self.char_rows = Some((key, rows.clone()));
        rows
    }

    fn char_table(&mut self, ui: &mut egui::Ui) {
        {
            let r = self.report.as_ref().unwrap();
            ui.horizontal(|ui| {
                ui.label("Filter:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.char_filter)
                        .hint_text("characters to find")
                        .desired_width(200.0),
                );
                anki_filter_combo(ui, &mut self.anki_filter, r.anki.is_some());
                ui.label(format!(
                    "{} unique characters",
                    fmt_int(r.char_freq.len() as u64)
                ));
            });
        }
        let rows = self.char_rows();
        let r = self.report.as_ref().unwrap();
        let status = r.anki.as_ref().map(|c| &c.char_status);
        let total = r.han_total.max(1) as f64;
        let sort_now = self.char_sort;
        let mut sort = None;
        TableBuilder::new(ui)
            // Cells otherwise inherit the top-down layout: text sits at the row top.
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .striped(true)
            .column(Column::exact(70.0))
            .column(Column::exact(70.0))
            .column(Column::exact(100.0))
            .column(Column::exact(90.0))
            .column(Column::exact(110.0))
            .column(Column::exact(if status.is_some() { 100.0 } else { 0.0 }))
            .column(Column::remainder())
            .header(24.0, |mut h| {
                let heads: [SortHead<CharCol>; 7] = [
                    (
                        "Rank",
                        Some((CharCol::Rank, false)),
                        "By frequency, most frequent first",
                    ),
                    ("Char", Some((CharCol::Char, false)), "By code point"),
                    ("Count", None, ""),
                    ("%", None, ""),
                    ("Cumulative %", None, ""),
                    (
                        if status.is_some() { "Status" } else { "" },
                        status.map(|_| (CharCol::Status, false)),
                        "Least known first, then by frequency",
                    ),
                    ("Code point", None, ""),
                ];
                for (t, c, hover) in heads {
                    h.col(|ui| match c {
                        Some((c, first_desc)) => {
                            if let Some(s) = sort_header(ui, t, hover, c, sort_now, first_desc) {
                                sort = Some(s);
                            }
                        }
                        None => {
                            ui.strong(t);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(26.0, rows.len(), |mut row| {
                    let i = rows[row.index()];
                    let (c, n) = r.char_freq[i];
                    row.col(|ui| {
                        ui.label((i + 1).to_string());
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(c.to_string()).size(20.0));
                    });
                    row.col(|ui| {
                        ui.label(fmt_int(n as u64));
                    });
                    row.col(|ui| {
                        ui.label(format!("{:.3}", n as f64 * 100.0 / total));
                    });
                    row.col(|ui| {
                        ui.label(format!("{:.2}", self.char_cum[i]));
                    });
                    row.col(|ui| {
                        if let Some(s) = status {
                            ui.label(status_text(s[i]));
                        }
                    });
                    row.col(|ui| {
                        ui.monospace(format!("U+{:04X}", c as u32));
                    });
                });
            });
        if let Some(s) = sort {
            self.char_sort = s;
        }
    }

    /// The Words table's rows (indexes into `word_freq`), as [`Self::char_rows`].
    fn word_rows(&mut self) -> Arc<Vec<usize>> {
        let key = RowsKey {
            generation: self.report_gen,
            filter: self.word_filter.trim().to_owned(),
            known: self.anki_filter,
            sort: self.word_sort,
        };
        if let Some((k, rows)) = &self.word_rows
            && *k == key
        {
            return rows.clone();
        }
        let r = self.report.as_ref().unwrap();
        let status = r.anki.as_ref().map(|c| &c.word_status);
        let f = key.filter.as_str();
        let mut rows: Vec<usize> = (0..r.word_freq.len())
            .filter(|&i| {
                let w = &r.word_freq[i].0;
                f.is_empty() || w.contains(f) || r.display(w).contains(f)
            })
            .filter(|&i| status.is_none_or(|s| key.known.matches(s[i])))
            .collect();
        let (col, desc) = key.sort;
        let word = |i: usize| r.display(&r.word_freq[i].0);
        match col {
            // Each length counted once, not at every comparison.
            WordCol::Length => {
                rows.sort_by_cached_key(|&i| {
                    let n = r.word_freq[i].0.chars().count();
                    (if desc { usize::MAX - n } else { n }, i)
                });
            }
            _ => rows.sort_by(|&a, &b| {
                let o = match col {
                    WordCol::Word => word(a).cmp(word(b)),
                    WordCol::Status => {
                        status.map_or(std::cmp::Ordering::Equal, |s| s[a].cmp(&s[b]))
                    }
                    _ => a.cmp(&b),
                };
                // Ties stay in frequency order.
                (if desc { o.reverse() } else { o }).then(a.cmp(&b))
            }),
        }
        let rows = Arc::new(rows);
        self.word_rows = Some((key, rows.clone()));
        rows
    }

    fn word_table(&mut self, ui: &mut egui::Ui) {
        /// Row height of the Words table.
        const ROW_H: f32 = 26.0;
        {
            let r = self.report.as_ref().unwrap();
            ui.horizontal(|ui| {
                ui.label("Filter:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.word_filter)
                        .hint_text("substring")
                        .desired_width(200.0),
                );
                anki_filter_combo(ui, &mut self.anki_filter, r.anki.is_some());
                ui.label(format!(
                    "{} unique words",
                    fmt_int(r.word_freq.len() as u64)
                ));
                ui.label(
                    RichText::new("↑ ↓ select a word · A marks it known or unmarks it").weak(),
                );
            });
        }
        let rows = self.word_rows();
        let r = self.report.as_ref().unwrap();
        let status = r.anki.as_ref().map(|c| &c.word_status);
        // Word length is counted in Chinese characters; Korean and Japanese
        // words have no length column.
        let show_len = r.language == Language::Chinese;
        let total = r.words_total.max(1) as f64;
        let known = &self.known.settings;
        // Looked up for every row drawn: a set, not the list.
        let marked: HashSet<&str> = known.marked.iter().map(String::as_str).collect();
        let sort_now = self.word_sort;
        let mut sort = None;
        // (word, mark) from a +/− click or A, applied after the table.
        let mut toggle: Option<(String, bool)> = None;
        // A on a word known from Anki or a list: why nothing happened.
        let mut note: Option<String> = None;

        // The selection's row in the filtered list.
        let word_of = |p: usize| &r.word_freq[rows[p]].0;
        let mut pos = self
            .word_sel
            .as_ref()
            .and_then(|sel| (0..rows.len()).find(|&p| word_of(p) == sel));
        let refill = std::mem::take(&mut self.word_sel_refill);
        if pos.is_none() && refill && !rows.is_empty() {
            pos = Some(self.word_sel_pos.min(rows.len() - 1));
        }
        // Keys, unless the filter box is being typed in.
        let mut moved = false;
        if !ui.ctx().egui_wants_keyboard_input() && !rows.is_empty() {
            let last = rows.len() - 1;
            let page = ((ui.available_height() / ROW_H) as isize - 2).max(1);
            let (home, end, steps, mark) = ui.input_mut(|i| {
                use egui::{Key, Modifiers};
                let mut n = |key| i.count_and_consume_key(Modifiers::NONE, key) as isize;
                let steps = n(Key::ArrowDown) - n(Key::ArrowUp)
                    + page * (n(Key::PageDown) - n(Key::PageUp));
                let (home, end) = (n(Key::Home) > 0, n(Key::End) > 0);
                (home, end, steps, i.consume_key(Modifiers::NONE, Key::A))
            });
            let mut target = pos;
            if home {
                target = Some(0);
            }
            if end {
                target = Some(last);
            }
            // With nothing selected, the first step selects the top row.
            if steps != 0 {
                target = Some(match target {
                    None => 0,
                    Some(p) => (p as isize + steps).clamp(0, last as isize) as usize,
                });
            }
            if home || end || steps != 0 {
                pos = target;
                moved = true;
                // The scroll to the row happens next frame; egui only draws on input.
                ui.ctx().request_repaint();
                // egui also moves keyboard focus between widgets with the
                // arrows (after a click on a tab or +/−); the list has them.
                ui.memory_mut(|m| {
                    if let Some(id) = m.focused() {
                        m.surrender_focus(id);
                    }
                });
            }
            if mark && let Some(p) = pos {
                let (i, w) = (rows[p], word_of(p));
                if marked.contains(w.as_str()) {
                    toggle = Some((w.clone(), false));
                } else if let Some(st) = status.map(|s| s[i]).filter(|s| s.is_known()) {
                    let from = if st == Status::Listed {
                        "a ticked level or word list"
                    } else {
                        "Anki"
                    };
                    note = Some(format!(
                        "{} is known from {from}; A only unmarks words you marked",
                        r.display(w)
                    ));
                } else {
                    toggle = Some((w.clone(), true));
                }
            }
        }
        // Clicking a row also selects it (below).
        let mut clicked: Option<usize> = None;

        let mut table = TableBuilder::new(ui)
            // Cells otherwise inherit the top-down layout: text sits at the row top.
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .striped(true)
            .sense(egui::Sense::click())
            .column(Column::exact(70.0))
            .column(Column::exact(30.0))
            // Long words are cut off at its edge; drag it wider to see them.
            .column(
                Column::initial(200.0)
                    .at_least(100.0)
                    .resizable(true)
                    .clip(true),
            )
            .column(Column::exact(100.0))
            .column(Column::exact(90.0))
            .column(Column::exact(110.0))
            .column(Column::exact(if status.is_some() { 100.0 } else { 0.0 }))
            .column(Column::remainder());
        if moved && let Some(p) = pos {
            table = table.scroll_to_row(p, None);
        }
        table
            .header(24.0, |mut h| {
                let heads: [SortHead<WordCol>; 8] = [
                    (
                        "Rank",
                        Some((WordCol::Rank, false)),
                        "By frequency, most frequent first",
                    ),
                    ("", None, ""),
                    (
                        "Word",
                        Some((WordCol::Word, false)),
                        "Alphabetically, by code point",
                    ),
                    ("Count", None, ""),
                    ("%", None, ""),
                    ("Cumulative %", None, ""),
                    (
                        if status.is_some() { "Status" } else { "" },
                        status.map(|_| (WordCol::Status, false)),
                        "Least known first, then by frequency",
                    ),
                    (
                        if show_len { "Length" } else { "" },
                        show_len.then_some((WordCol::Length, true)),
                        "Longest first, then by frequency",
                    ),
                ];
                for (t, c, hover) in heads {
                    h.col(|ui| match c {
                        Some((c, first_desc)) => {
                            if let Some(s) = sort_header(ui, t, hover, c, sort_now, first_desc) {
                                sort = Some(s);
                            }
                        }
                        None => {
                            ui.strong(t);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_H, rows.len(), |mut row| {
                    let i = rows[row.index()];
                    let (w, n) = &r.word_freq[i];
                    row.set_selected(pos == Some(row.index()));
                    row.col(|ui| {
                        ui.label((i + 1).to_string());
                    });
                    row.col(|ui| {
                        let is_known = status.is_some_and(|s| s[i].is_known());
                        if marked.contains(w.as_str()) {
                            if ui
                                .small_button("−")
                                .on_hover_text("You marked this word known. Click to unmark it.")
                                .clicked()
                            {
                                toggle = Some((w.clone(), false));
                            }
                        } else if !is_known
                            && ui
                                .small_button("+")
                                .on_hover_text(
                                    "Mark as known (added to your known words, kept between runs)",
                                )
                                .clicked()
                        {
                            toggle = Some((w.clone(), true));
                        }
                    });
                    row.col(|ui| {
                        let shown = r.display(w);
                        let l = ui.label(RichText::new(shown).size(18.0));
                        if shown != w {
                            l.on_hover_text(format!(
                                "Counted as {w}, which merges its spellings and forms"
                            ));
                        }
                    });
                    row.col(|ui| {
                        ui.label(fmt_int(*n as u64));
                    });
                    row.col(|ui| {
                        ui.label(format!("{:.3}", *n as f64 * 100.0 / total));
                    });
                    row.col(|ui| {
                        ui.label(format!("{:.2}", self.word_cum[i]));
                    });
                    row.col(|ui| {
                        if let Some(s) = status {
                            ui.label(status_text(s[i]));
                        }
                    });
                    row.col(|ui| {
                        if show_len {
                            ui.label(w.chars().count().to_string());
                        }
                    });
                    if row.response().clicked() {
                        clicked = Some(row.index());
                    }
                });
            });
        if clicked.is_some() {
            pos = clicked;
        }
        if let Some(p) = pos {
            self.word_sel = Some(word_of(p).clone());
            self.word_sel_pos = p;
        }
        if let Some(s) = sort {
            self.word_sort = s;
        }
        if toggle.is_some() {
            // If the word leaves the list ("Not known"), the next one takes its place.
            self.word_sel_refill = true;
        }
        if let Some(m) = note {
            self.toast = Some((m, ui.input(|i| i.time) + 4.0));
        }
        if let Some((w, on)) = toggle {
            self.mark_known(&w, on, ui);
        }
    }

    /// Mark `w` known (or unmark it) from the Words or Suggested words tab.
    fn mark_known(&mut self, w: &str, on: bool, ui: &egui::Ui) {
        if !self.known.settings.set_marked(w, on) {
            return;
        }
        let shown = self
            .report
            .as_ref()
            .map_or(w.to_owned(), |r| r.display(w).to_owned());
        let msg = if on {
            format!("Marked {shown} as known")
        } else {
            format!("Unmarked {shown}")
        };
        self.toast = Some((msg, ui.input(|i| i.time) + 3.0));
        self.known_changed();
    }

    fn sections(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Untick sections to exclude them (cover, copyright, TOC, ads…). Totals update instantly.");
            if ui.button("Include all").clicked() {
                self.included.iter_mut().for_each(|x| *x = true);
                changed = true;
            }
            if ui.button("Defaults").clicked()
                && let Some(b) = &self.book
            {
                self.included = b.sections.iter().map(|s| s.default_included()).collect();
                changed = true;
            }
        });
        let r = self.report.as_ref().unwrap();
        let n = r.sections.len();
        // A Korean book has word columns only (a Japanese one counts kanji).
        let chinese = r.language != Language::Korean;
        let skip = if chinese { 0 } else { 3 };
        let mut toggles: Vec<usize> = Vec::new();
        TableBuilder::new(ui)
            // Cells otherwise inherit the top-down layout: text sits at the row top.
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .striped(true)
            .column(Column::exact(28.0))
            .column(Column::exact(44.0))
            .column(
                Column::initial(300.0)
                    .at_least(120.0)
                    .resizable(true)
                    .clip(true),
            )
            .column(Column::exact(80.0))
            .columns(Column::exact(80.0), 6 - skip)
            .header(24.0, |mut h| {
                let heads = [
                    "",
                    "#",
                    "Section",
                    "Kind",
                    "Chars",
                    "Unique",
                    "New",
                    "Words",
                    "Unique w.",
                    "New w.",
                ];
                for (j, t) in heads.into_iter().enumerate() {
                    if (4..4 + skip).contains(&j) {
                        continue;
                    }
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, n, |mut row| {
                    let i = row.index();
                    let s = &r.sections[i];
                    row.col(|ui| {
                        let mut v = s.included;
                        if ui.checkbox(&mut v, "").changed() {
                            toggles.push(i);
                        }
                    });
                    row.col(|ui| {
                        ui.label(i.to_string());
                    });
                    row.col(|ui| {
                        let t = RichText::new(&s.label);
                        ui.label(if s.included {
                            t
                        } else {
                            t.weak().strikethrough()
                        });
                    });
                    row.col(|ui| {
                        ui.label(match s.kind {
                            SectionKind::Body => "",
                            SectionKind::Nav => "TOC",
                            SectionKind::NonLinear => "non-linear",
                            SectionKind::FrontMatter => "front",
                        });
                    });
                    let dash = |o: Option<usize>| {
                        o.map(|v| fmt_int(v as u64)).unwrap_or_else(|| "–".into())
                    };
                    let values = [
                        fmt_int(s.han),
                        fmt_int(s.unique_han as u64),
                        dash(s.new_han),
                        fmt_int(s.words),
                        fmt_int(s.unique_words as u64),
                        dash(s.new_words),
                    ];
                    for v in values.into_iter().skip(skip) {
                        row.col(|ui| {
                            ui.label(v);
                        });
                    }
                });
            });
        for i in toggles {
            self.included[i] = !self.included[i];
            changed = true;
        }
        if changed {
            self.rebuild_report();
        }
    }

    fn suggestions_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.forced_words_ui(ui, ctx);
        ui.separator();
        let Some(cands) = &self.suggestions else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Looking for book-specific words…");
            });
            return;
        };
        ui.label(
            "Strings that recur in this book, behave like single words (their parts rarely occur apart; many different \
             characters appear on either side), but are split by the segmenter or glued to a neighbour (蒋舸没). These are \
             mostly character names, places and in-world terms. Pre-ticked = high confidence (unknown to a general dictionary, \
             and either the segmenter already gets it right part of the time or the statistical evidence is very strong). \
             Applying forces the ticked strings to always count as one word; forced words are saved and apply to every book. \
             A word missing here can be typed in above.",
        );
        ui.add_space(4.0);
        let n_sel = self.selected.len();
        let mut apply = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    n_sel > 0 && self.job.is_none(),
                    egui::Button::new(format!("✔ Apply {n_sel} selected")),
                )
                .clicked()
            {
                apply = true;
            }
            if ui.button("Select recommended").clicked() {
                self.selected = cands
                    .iter()
                    .filter(|c| c.recommended)
                    .map(|c| c.word.clone())
                    .collect();
            }
            if ui.button("Select none").clicked() {
                self.selected.clear();
            }
            ui.label(format!(
                "{} candidates, {} recommended{}",
                cands.len(),
                cands.iter().filter(|c| c.recommended).count(),
                if self.user_words.is_empty() {
                    String::new()
                } else {
                    format!(" · {} words already forced", self.user_words.len())
                }
            ));
            info_button(ui, |ui| {
                egui::Grid::new("suggest-columns")
                    .num_columns(2)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        for (t, d) in SUGGEST_COLUMNS {
                            ui.strong(t);
                            ui.label(d);
                            ui.end_row();
                        }
                    });
            });
        });
        let mut toggles: Vec<String> = Vec::new();
        // (word, mark) from a +/− click, applied after the table.
        let mut mark: Option<(String, bool)> = None;
        let marked: HashSet<&str> = self
            .known
            .settings
            .marked
            .iter()
            .map(String::as_str)
            .collect();
        let merged = self.known.merged.as_deref();
        TableBuilder::new(ui)
            // Cells otherwise inherit the top-down layout: text sits at the row top.
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .striped(true)
            .column(Column::exact(28.0))
            .column(Column::exact(30.0))
            .column(Column::exact(150.0))
            .column(Column::exact(if merged.is_some() { 100.0 } else { 0.0 }))
            .column(Column::exact(100.0))
            .column(Column::exact(110.0))
            .column(Column::exact(100.0))
            .column(Column::exact(110.0))
            .column(Column::remainder())
            .header(24.0, |mut h| {
                for (t, tip) in [
                    ("", ""),
                    ("", ""),
                    ("Word", ""),
                    (
                        if merged.is_some() { "Status" } else { "" },
                        "Whether you know the word (Anki, the levels and word lists ticked, or marked with +)",
                    ),
                    SUGGEST_COLUMNS[0],
                    SUGGEST_COLUMNS[1],
                    SUGGEST_COLUMNS[2],
                    SUGGEST_COLUMNS[3],
                    ("", ""),
                ] {
                    h.col(|ui| {
                        let r = ui.strong(t);
                        if !tip.is_empty() {
                            r.on_hover_text(tip);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(26.0, cands.len(), |mut row| {
                    let c = &cands[row.index()];
                    row.col(|ui| {
                        let mut v = self.selected.contains(&c.word);
                        if ui.checkbox(&mut v, "").changed() {
                            toggles.push(c.word.clone());
                        }
                    });
                    let st = merged.map(|k| k.word(&c.word));
                    row.col(|ui| {
                        if marked.contains(c.word.as_str()) {
                            if ui
                                .small_button("−")
                                .on_hover_text("You marked this word known. Click to unmark it.")
                                .clicked()
                            {
                                mark = Some((c.word.clone(), false));
                            }
                        } else if !st.is_some_and(Status::is_known)
                            && ui
                                .small_button("+")
                                .on_hover_text(
                                    "Mark as known (added to your known words, kept between runs). To keep a name whole, apply it instead.",
                                )
                                .clicked()
                        {
                            mark = Some((c.word.clone(), true));
                        }
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&c.word).size(18.0));
                    });
                    row.col(|ui| {
                        if let Some(s) = st {
                            ui.label(status_text(s));
                        }
                    });
                    row.col(|ui| {
                        ui.label(fmt_int(c.occurrences as u64));
                    });
                    row.col(|ui| {
                        ui.label(format!(
                            "{} ({:.0}%)",
                            fmt_int(c.split_occurrences() as u64),
                            c.split_occurrences() as f64 * 100.0 / c.occurrences as f64
                        ));
                    });
                    row.col(|ui| {
                        ui.label(format!("{:.2}", c.dependency));
                    });
                    row.col(|ui| {
                        ui.label(format!("{:.1}", c.left_entropy.min(c.right_entropy)));
                    });
                    row.col(|ui| {
                        if c.recommended {
                            ui.label(RichText::new("recommended").color(palette::good()));
                        } else if c.in_dictionary {
                            ui.label(RichText::new("general word").weak())
                                .on_hover_text("In a general dictionary (jieba's or CC-CEDICT): the split is usually a granularity choice, not an error.");
                        } else if c.split_occurrences() == c.occurrences {
                            ui.label(RichText::new("always split").weak())
                                .on_hover_text("The segmenter never keeps this whole: often a set phrase or a compound its standard splits.");
                        }
                    });
                });
            });
        for w in toggles {
            if !self.selected.remove(&w) {
                self.selected.insert(w);
            }
        }
        if let Some((w, on)) = mark {
            self.mark_known(&w, on, ui);
        }
        if apply {
            self.apply_selected(ctx);
        }
    }

    fn notes(&mut self, ui: &mut egui::Ui) {
        let r = self.report.as_ref().unwrap();
        let from_cache = self.prep.as_ref().is_some_and(|p| p.cached);
        let mut clear_cache = false;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.heading("About this file");
                for n in &r.notes {
                    ui.label(format!("• {n}"));
                }
                if r.user_words > 0 {
                    ui.label(format!(
                        "• User dictionary: {} words applied.",
                        r.user_words
                    ));
                }
                if r.known_words > 0 {
                    ui.label(format!(
                        "• Imported word lists: {} words used to join words the segmenter split, never cutting through one of its words. Changes apply on returning to the Overview.",
                        fmt_int(r.known_words as u64)
                    ));
                }
                ui.add_space(12.0);
                ui.heading("Models");
                for m in [
                    &hanstat::models::LTP,
                    &hanstat::models::CKIP,
                    &hanstat::models::KIWI,
                    hanstat::models::sudachi_local(),
                ] {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "• {} — {}",
                            m.title,
                            if m.is_cached() {
                                "downloaded"
                            } else {
                                "not downloaded yet"
                            }
                        ));
                        ui.hyperlink_to("source", m.homepage);
                    });
                    ui.label(RichText::new(format!("   Licence: {}", m.licence)).weak());
                }
                ui.label(
                    RichText::new(format!(
                        "   Cache: {}",
                        hanstat::models::cache_root().display()
                    ))
                    .weak(),
                );
                ui.add_space(12.0);
                ui.heading("Saved analyses");
                match &self.cache {
                    Some(cache) => {
                        ui.label(if from_cache {
                            "• This book's counts were read from an earlier analysis of the same text with the same segmenter, model and words."
                        } else {
                            "• This book was analysed just now; its counts are saved for the next time it is opened."
                        });
                        ui.label("• A saved analysis is used only if the text, the segmenter and its model, the forced and known words and hanstat's analysis code are all unchanged. Including or excluding sections, Anki and vocabulary levels never need a new analysis.");
                        ui.horizontal(|ui| {
                            let (n, bytes) = self.cache_usage;
                            ui.label(format!(
                                "• {} saved ({:.1} MB of at most {:.0} MB; the ones unused the longest are deleted first)",
                                fmt_int(n as u64),
                                bytes as f64 / 1e6,
                                hanstat::cache::MAX_BYTES as f64 / 1e6
                            ));
                            if ui
                                .add_enabled(n > 0, egui::Button::new("Delete all").small())
                                .on_hover_text("Books are analysed again the next time they are opened.")
                                .clicked()
                            {
                                clear_cache = true;
                            }
                        });
                        ui.label(
                            RichText::new(format!("   Folder: {}", cache.dir().display())).weak(),
                        );
                    }
                    None => {
                        ui.label("• Off (HANSTAT_NO_CACHE is set): every book is analysed afresh.");
                    }
                }
                ui.add_space(12.0);
                ui.heading("How things are counted");
                let method = match r.language {
                    Language::Chinese => METHOD,
                    Language::Korean => KOREAN_METHOD,
                    Language::Japanese => JAPANESE_METHOD,
                };
                for line in method {
                    ui.label(format!("• {line}"));
                }
            });
        if clear_cache && let Some(cache) = self.cache.clone() {
            if let Err(e) = cache.clear() {
                self.set_error("cache", e);
            }
            self.cache_usage = cache.usage();
        }
    }
}

/// The Suggested words table's numeric columns and what they mean.
const SUGGEST_COLUMNS: [(&str, &str); 4] = [
    (
        "Occurrences",
        "Times the string occurs in the included sections",
    ),
    (
        "Wrong",
        "Occurrences the segmenter did NOT keep as one word: split apart, or glued to a neighbour (蒋舸没)",
    ),
    (
        "Dependency",
        "Share of the rarer part's occurrences that are inside this string (1.0 = parts never occur apart)",
    ),
    (
        "Boundary freedom",
        "Min of left/right neighbour entropy (nats); high = appears in many contexts",
    ),
];

const KOREAN_METHOD: &[&str] = &[
    "Language: a book is Korean when it has more Hangul than Chinese characters. The badge at the top right shows the mode; Anki decks and fields, word lists, marked words and the user dictionary are kept separately for Chinese and Korean.",
    "A Korean book is measured in words only: there are no character or syllable counts, and lengths and reading time are in words.",
    "Words are dictionary forms from Kiwi, a Korean morphological analyser: irregular stems are restored (걸어 → 걷다, 도와 → 돕다), predicates are counted with 다 (먹었어요 → 먹다), and a noun, root or adverb with the 하/되 suffix is one word (공부했다 → 공부하다, 깨끗해 → 깨끗하다). Particles, endings, the copula 이다 and affixes are grammar, not words. Hanja, Latin words and numbers are not Korean words.",
    "Kiwi chooses between readings from context (들어요 as 듣다 or 들다, 물었다 as 묻다 or 물다). In a test on two novels it was right in about four of five places where it disagreed with mecab-ko-dic. Compound verbs are sometimes split into their parts (들려오다 → 들리다 + 오다).",
    "Names: Kiwi keeps most names whole, but may split one that is also made of common words. Words in the user dictionary are added to Kiwi as proper nouns and are always kept whole.",
    "EPUB: only documents in the spine are read, in reading order, each once. <head>, scripts, styles, ruby annotations (<rt>/<rp>), hidden elements and image alt text are excluded. Block elements and <br> end paragraphs. Line breaks in the HTML source between Hangul become spaces, as browsers render them.",
    "TXT: encoding from BOM, strict UTF-8 validation, UTF-16 statistics, then chardetng (EUC-KR is read as CP949). Hard-wrapped lines are joined with a space. Chapter headings like 제1장, 12화, 프롤로그 and 외전 start sections.",
    "Sentences end at . ? ! … (a full stop counts when a space, quote or the paragraph end follows) or the end of a paragraph.",
    "Coverage = the number of most-frequent distinct words whose occurrences add up to X% of all word occurrences.",
    "Anki: the collection is copied (with its write-ahead log) to a temporary folder and read from there, so it works while Anki is open and is never modified. A review card is mature at an interval of 21+ days and young below that, or, with \"Mature by FSRS stability\" on, at the chosen stability (cards without FSRS data keep the interval rule). Every ticked field is analysed with Kiwi and the user dictionary, so a card for 먹다 or 먹어요 matches the book's 먹다.",
];

const JAPANESE_METHOD: &[&str] = &[
    "Language: a book is Japanese when its kana are at least a tenth as many as its kanji (a Chinese book has at most a few kana). The badge at the top right shows the mode; Anki decks and fields, word lists, marked words and the user dictionary are kept separately for each language.",
    "Characters are kanji (Script=Han letters), counted exactly as in Chinese books; kana are letters. Sentence and paragraph lengths are in words. Reading time is in characters per minute, where the characters are kanji and kana; punctuation, digits and Latin letters are not counted.",
    "Words come from Sudachi, a Japanese morphological analyser, with SudachiDict full in its long-unit mode C: compounds a dictionary lists whole stay whole (事務所, 一生懸命, 冒険者). Each word is counted by Sudachi's normalized form, which merges inflections and spellings (わかった, 分かる → 分かる; かわいく → 可愛い; ウソ → うそ), and shown as the book usually spells it. Two different words the book spells alike are told apart by their normalized form: いる (要る).",
    "Particles, auxiliaries (た, ます, ない…), punctuation, numbers in digits and Latin words are not Japanese words. Numerals in kanji keep their spelling (二十四).",
    "Before analysis, dialogue noise that Sudachi misreads is removed: an emphatic small っ at the end of a word (ですっ」), stretched vowels and long marks after two or more kana (ますぅ, まーす), and stutters (い、いいんですか; こ、これ). Short interjections such as はぁ, えー and えーと are kept. On five novels this changed Sudachi's reading in a few hundred places, mostly for the better (49 of 81 judged by hand; 6 worse, all stretched interjections like あはー).",
    "Sudachi was chosen over UniDic analysers (Lindera, Vibrato) after a comparison on five novels: it keeps character names whole (UniDic splits most invented names) and gives learner-sized words; on other words the two were about even. Words in the user dictionary are added to Sudachi as proper nouns and are always kept whole.",
    "Known words: a long word that is not known itself counts as known when all its short-unit parts are (冒険者 with 冒険 and 者 known), with the weakest of their statuses. The JLPT levels are Jonathan Waller's N5–N1 lists (as used by jisho.org), keyed by Sudachi's normalized form, so they match however the book writes a word.",
    "EPUB: only documents in the spine are read, in reading order, each once. <head>, scripts, styles, ruby annotations (<rt>/<rp>, so furigana is not counted), hidden elements and image alt text are excluded. Block elements and <br> end paragraphs. Line breaks in the HTML source between Japanese characters are removed, as browsers do.",
    "TXT: encoding from BOM, strict UTF-8 validation, UTF-16 statistics, then chardetng. Each line is a paragraph unless the file is detected as hard-wrapped.",
    "Sentences end at 。！？!?… (runs count once) or the end of a paragraph.",
    "Coverage = the number of most-frequent distinct words (kanji) whose occurrences add up to X% of all word (kanji) occurrences.",
    "Anki: the collection is copied (with its write-ahead log) to a temporary folder and read from there, so it works while Anki is open and is never modified. A review card is mature at an interval of 21+ days and young below that, or, with \"Mature by FSRS stability\" on, at the chosen stability (cards without FSRS data keep the interval rule). Every ticked field is analysed with Sudachi and the user dictionary, with furigana readings (会[あ]う) removed, so a card for 分かる, わかる or わかった matches the book's 分かる. A kanji is known if it is written in a known word on a card.",
];

const METHOD: &[&str] = &[
    "Chinese character = a code point with Unicode Script=Han that is a letter (all CJK Unified Ideograph blocks incl. Extensions A–J and supplementary planes, compatibility ideographs, 〇, 々). Punctuation, radicals, Latin, digits, whitespace are never counted as Chinese characters.",
    "Text is NFC-normalised first: compatibility ideographs become their unified form, Kangxi radicals used as characters are folded, variation selectors are dropped (葛󠄀 and 葛 are one character), invisible characters (zero-width space, soft hyphen, BOM) are removed.",
    "EPUB: only documents in the spine are read, in reading order, each once. <head>, scripts, styles, ruby annotations (<rt>/<rp>), hidden elements and image alt text are excluded. Block elements and <br> end paragraphs; inline tags never split words. Line breaks in the HTML source between Chinese characters are removed, as browsers do.",
    "TXT: encoding from BOM, strict UTF-8 validation, UTF-16 statistics, then chardetng (GBK is read as GB18030). Each line is a paragraph unless the file is detected as hard-wrapped.",
    "Words: there is no single correct segmentation of Chinese; different standards (PKU, CTB, MSR, AS) disagree on 5–15% of boundaries, and unique-word counts can differ by 20–40% between segmenters. Only tokens containing Chinese characters are counted as Chinese words. Character counts, by contrast, are exact.",
    "Segmenters (SIGHAN 2005 F1): LTP perceptron is best on Simplified text (MSR 87.7, PKU 95.2) and takes seconds; CKIP BERT is best on Traditional text (CityU 90.1, AS 97.9) and keeps character names together more consistently, but takes minutes per novel; jieba is fastest and least accurate (81–84). Auto picks LTP for Simplified and CKIP for Traditional.",
    "Proper names are the main source of segmentation errors. Apply them from the Suggested words tab to force them as single words; forced words apply to every segmenter.",
    "Script folding for the segmenter only: Traditional text is folded to Simplified for LTP/jieba (character-level), and Simplified text to Traditional for CKIP (OpenCC phrase-level, e.g. 以后→以後), matching each model's training script. The conversion is length-preserving and all counts and lists keep the original characters.",
    "Sentences end at 。！？!?… (runs count once) or the end of a paragraph; a sentence must contain a letter or digit.",
    "Coverage = the number of most-frequent distinct characters/words whose occurrences add up to X% of all occurrences.",
    "Anki: the collection is copied (with its write-ahead log) to a temporary folder and read from there, so it works while Anki is open and is never modified. A review card is mature at an interval of 21+ days and young below that, or, with \"Mature by FSRS stability\" on, at the chosen stability (cards without FSRS data keep the interval rule); learning and relearning cards are 'learning'. A note takes its best card's status in the selected decks, a word its best note's, and a character the best status of any known word containing it. A ticked field value with sentence punctuation is split into words with the book's segmenter and forced words, so its boundaries agree with the book's. Otherwise each run of Chinese characters is a word; runs longer than 4 characters (idioms, sayings) also count their segmented parts. Book words must match exactly (after optional Traditional→Simplified folding), so the segmenter's word boundaries matter.",
];

/// The Library search box's hover text.
const LIBRARY_SEARCH_HOVER: &str = "Filters by title and file name. Letters need only appear in order (hp finds Harry Potter); \
     several words must all match. With * (any characters) or ? (one character) a word must match the whole title or \
     file name: 三体* starts with 三体, *.txt ends with .txt. Case is ignored and 三體 finds 三体. Esc clears it.";

impl App {
    /// Every book analysed before, as recorded when its analysis (or what
    /// counts as known) last changed. A click on a title opens the book.
    fn history_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.load_read_marks();
        ui.horizontal_wrapped(|ui| {
            ui.heading("Library");
            ui.add_space(8.0);
            ui.selectable_value(&mut self.history_tab, HistTab::Books, "Books")
                .on_hover_text("Every book analysed, with its statistics");
            ui.selectable_value(&mut self.history_tab, HistTab::Path, "Reading path")
                .on_hover_text(
                    "An order to read the books in, where the words you study for one book make the next ones easier",
                );
            ui.selectable_value(&mut self.history_tab, HistTab::Stats, "Stats")
                .on_hover_text("The library as a whole: its size, its vocabulary and how much of it you know");
            ui.add_space(8.0);
            if self.history_tab != HistTab::Stats {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.library_filter)
                        .hint_text("🔍 Search")
                        .desired_width(180.0),
                )
                .on_hover_text(LIBRARY_SEARCH_HOVER);
                if (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.library_filter.clear();
                }
                info_button(ui, |ui| {
                    ui.label(LIBRARY_SEARCH_HOVER);
                });
                if !self.library_filter.is_empty()
                    && ui.small_button("✖").on_hover_text("Clear the search").clicked()
                {
                    self.library_filter.clear();
                }
                if self.history_tab == HistTab::Books
                    && Language::ALL
                        .into_iter()
                        .any(|l| self.known_of(l).is_some_and(|k| !k.read.is_empty()))
                {
                    ui.checkbox(&mut self.library_hide_read, "Hide read")
                        .on_hover_text("Leave out the books marked read");
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can = self
                    .history
                    .entries
                    .iter()
                    .filter(|e| self.exists.get(&e.path))
                    .count();
                let missing = self.history.entries.len() - can;
                let mut hover = String::from(
                    "Analyse every book in the Library again in the background, with the current known words, user dictionary and settings, \
                     each with the sections and segmenter it was last analysed with. \
                     Rows are also updated on their own when any of these change; this redoes them all.",
                );
                if missing > 0 {
                    hover.push_str(&format!(
                        " {missing} {} no longer where {} were and keep their rows.",
                        if missing == 1 { "book is" } else { "books are" },
                        if missing == 1 { "it" } else { "they" },
                    ));
                }
                let busy = self.batch.as_ref().is_some_and(|b| b.mode != BatchMode::Auto);
                let r = ui
                    .add_enabled(!busy && can > 0, egui::Button::new("⟳ Recalculate library"))
                    .on_hover_text(hover)
                    .on_disabled_hover_text(if busy {
                        "Books are being analysed: wait for them, or stop them in the status bar"
                    } else {
                        "No book in the Library is where it was analysed"
                    });
                if r.clicked() {
                    self.recalculate_library(ctx);
                }
            });
        });
        if let Some(e) = &self.history_error {
            let mut dismiss = false;
            ui.horizontal(|ui| {
                dismiss = ui.small_button("✖").on_hover_text("Dismiss").clicked();
                ui.colored_label(palette::warn(), format!("⚠ {e}"));
            });
            if dismiss {
                self.history_error = None;
            }
        }
        ui.separator();
        match self.history_tab {
            HistTab::Path => return self.plan_view(ui, ctx),
            HistTab::Stats => return self.stats_view(ui, ctx),
            HistTab::Books => {}
        }
        if self.history.entries.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(if drop_works() {
                        "No books analysed yet. Open or drop an .epub or .txt file."
                    } else {
                        "No books analysed yet. Open an .epub or .txt file."
                    })
                    .size(22.0)
                    .weak(),
                );
            });
            return;
        }

        // Reading time at the speeds set on the Overview, as there.
        let sp = &self.settings;
        let (cpm, ja_cpm, wpm) = (sp.cpm as f64, sp.ja_cpm as f64, sp.wpm as f64);
        let minutes = |s: &history::Stats| match s.language {
            Language::Chinese => s.han_total as f64 / cpm,
            Language::Japanese => (s.han_total + s.kana_total) as f64 / ja_cpm,
            Language::Korean => s.words_total as f64 / wpm,
        };
        let (col, desc) = self.history_sort;
        let style_on = self.style_on();
        let style_scores: HashMap<&str, f32> = if style_on {
            self.style.store.current()
        } else {
            HashMap::new()
        };
        let style_scale = self.style_scale();
        let style_skipped = &self.style.skipped;
        let style_off: HashMap<&str, f64> = self
            .history
            .entries
            .iter()
            .map(|e| (e.id.as_str(), self.style_offset(&e.id)))
            .collect();
        let read_at = self.read_dates();
        let hide_read = self.library_hide_read;
        let hist = &self.history;
        let entries = &hist.entries;
        let covers = &mut self.covers;
        let exists = &self.exists;
        let key = |i: usize| -> f64 {
            let s = &entries[i].stats;
            match col {
                HistCol::Chars => s.han_total as f64,
                HistCol::Words => s.words_total as f64,
                HistCol::Unique => s.words_unique as f64,
                // Books with nothing known sort below every percentage.
                HistCol::Known => s.known.as_ref().map_or(-1.0, |k| k.running_words),
                // Harder first when descending: the share known as counted for
                // the difficulty (writing style included), the lower the harder.
                // Books with nothing known sort below every book with a difficulty.
                HistCol::Difficulty => s.known.as_ref().map_or(-1.0, |k| {
                    let off = style_off
                        .get(entries[i].id.as_str())
                        .copied()
                        .unwrap_or(0.0);
                    100.0 - (k.running_words - off)
                }),
                HistCol::Time => minutes(s),
                HistCol::Analysed => entries[i].analysed as f64,
                // Books not read sort below every date.
                HistCol::Read => read_at.get(&entries[i].id).map_or(-1.0, |&t| t as f64),
                // Books not rated sort below every rating.
                HistCol::Style => style_scores
                    .get(entries[i].id.as_str())
                    .map_or(-1.0, |&x| x as f64),
                HistCol::Title | HistCol::Language => 0.0,
            }
        };
        let lang_key = |i: usize| {
            (
                entries[i].stats.language as u8,
                entries[i].stats.variant.as_str(),
            )
        };
        let query = search::Query::new(&self.library_filter);
        let mut rows: Vec<usize> = (0..entries.len())
            .filter(|&i| book_matches(&query, &entries[i].title, &entries[i].path))
            .filter(|&i| !hide_read || !read_at.contains_key(&entries[i].id))
            .collect();
        match col {
            // Each title lowercased once, not at every comparison.
            HistCol::Title => {
                rows.sort_by_cached_key(|&i| entries[i].title.to_lowercase());
                if desc {
                    rows.reverse();
                }
            }
            _ => rows.sort_by(|&a, &b| {
                let o = match col {
                    HistCol::Language => lang_key(a).cmp(&lang_key(b)),
                    _ => key(a).total_cmp(&key(b)),
                };
                if desc { o.reverse() } else { o }
            }),
        }

        if rows.is_empty() {
            ui.label(
                RichText::new(if hide_read && self.library_filter.is_empty() {
                    "Every book is marked read."
                } else {
                    "No book matches the search."
                })
                .weak(),
            );
            return;
        }
        let now = history::now();
        let mut open: Option<PathBuf> = None;
        let mut remove: Option<String> = None;
        let mut mark_read: Option<String> = None;
        let mut sort = None;
        let dash = || RichText::new("–").weak();
        TableBuilder::new(ui)
            // Cells otherwise inherit the top-down layout: text sits at the row top.
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .striped(true)
            .column(
                // Narrower while the writing-style column is shown, so the row still fits.
                Column::initial(if style_on { 140.0 } else { 300.0 })
                    .at_least(140.0)
                    .resizable(true)
                    .clip(true),
            )
            .column(Column::exact(90.0))
            .columns(Column::exact(100.0), 3)
            .column(Column::exact(80.0))
            .column(Column::exact(100.0))
            .columns(Column::exact(130.0), style_on as usize)
            .column(Column::exact(80.0))
            .column(Column::exact(92.0))
            .column(Column::exact(36.0))
            .column(Column::remainder())
            .header(24.0, |mut h| {
                let mut heads: Vec<(Option<HistCol>, &str, &str)> = vec![
                    (Some(HistCol::Title), "Book", "The book's title, or its file name"),
                    (Some(HistCol::Language), "Language", "Language, and script for Chinese"),
                    (
                        Some(HistCol::Chars),
                        "Characters",
                        "Chinese characters, or kanji in a Japanese book (Korean books have none)",
                    ),
                    (Some(HistCol::Words), "Words", "Running words, as the segmenter counted them"),
                    (Some(HistCol::Unique), "Unique words", "Distinct words"),
                    (
                        Some(HistCol::Known),
                        "Known",
                        "Running words you knew (Anki cards and the levels and word lists marked known) at the last analysis; names are left out",
                    ),
                    (Some(HistCol::Difficulty), "Difficulty", "From the share of running words known (and the writing style, while rated), as on the Overview"),
                    (Some(HistCol::Time), "Reading", "Reading time at the speeds set on the Overview"),
                    (Some(HistCol::Analysed), "Analysed", "When the book was last analysed"),
                    (
                        Some(HistCol::Read),
                        "Read",
                        "Mark a book read to count its most frequent words as known and take it off the reading path",
                    ),
                    (None, "", ""),
                ];
                if style_on {
                    heads.insert(
                        7,
                        (
                            Some(HistCol::Style),
                            "Writing style",
                            "How hard the prose is when you know its words, rated by a local AI model (⚙). The percentage compares it with the Library's rated books (50% is their median); the word is the model's own judgment. Chinese books only",
                        ),
                    );
                }
                for (c, t, hover) in heads {
                    h.col(|ui| {
                        let Some(c) = c else {
                            ui.strong(t).on_hover_text(hover);
                            return;
                        };
                        // Numbers and dates start largest first, text A–Z.
                        let first_desc = !matches!(c, HistCol::Title | HistCol::Language);
                        if let Some(s) = sort_header(ui, t, hover, c, (col, desc), first_desc) {
                            sort = Some(s);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(COVER_ROW, rows.len(), |mut row| {
                    let e = &entries[rows[row.index()]];
                    let s = &e.stats;
                    row.col(|ui| {
                        let tex = covers
                            .entry(e.id.clone())
                            .or_insert_with(|| load_cover(ctx, &hist.cover(&e.id)));
                        let (w, h) = COVER_SIZE;
                        match tex {
                            Some(t) => {
                                // Fit the box, keeping the cover's proportions.
                                let [tw, th] = t.size().map(|x| x as f32);
                                let k = (w / tw).min(h / th);
                                ui.allocate_ui(egui::vec2(w, h), |ui| {
                                    ui.centered_and_justified(|ui| {
                                        ui.add(
                                            egui::Image::new(&*t)
                                                .fit_to_exact_size(egui::vec2(tw * k, th * k))
                                                .corner_radius(2.0),
                                        )
                                        .on_hover_ui(|ui| {
                                            let k = 2.5 * k;
                                            ui.add(
                                                egui::Image::new(&*t)
                                                    .fit_to_exact_size(egui::vec2(tw * k, th * k)),
                                            );
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
                            if s.language != Language::Korean {
                                hover.push_str(&format!(
                                    ", {:.1}% of unique characters",
                                    k.unique_chars
                                ));
                            }
                        }
                        if s.words_for_98 > 0 {
                            hover.push_str(&format!(
                                "\n{} most frequent words cover 98% of the text",
                                fmt_int(s.words_for_98)
                            ));
                        }
                        if exists.get(&e.path) {
                            if ui.link(&e.title).on_hover_text(hover).clicked() {
                                open = Some(e.path.clone());
                            }
                        } else {
                            ui.label(RichText::new(&e.title).weak())
                                .on_hover_text(format!("{hover}\n\nThe file is no longer there."));
                        }
                    });
                    row.col(|ui| {
                        let variant = match s.variant.as_str() {
                            "Simplified" => Some(Variant::Simplified),
                            "Traditional" => Some(Variant::Traditional),
                            "Mixed" => Some(Variant::Mixed),
                            _ => None,
                        };
                        ui.label(mode_badge(s.language, variant).0);
                    });
                    row.col(|ui| {
                        if s.language == Language::Korean {
                            ui.label(dash());
                        } else {
                            ui.label(fmt_int(s.han_total));
                        }
                    });
                    row.col(|ui| {
                        ui.label(fmt_int(s.words_total));
                    });
                    row.col(|ui| {
                        ui.label(fmt_int(s.words_unique));
                    });
                    match &s.known {
                        Some(k) => {
                            row.col(|ui| {
                                ui.label(format!("{:.1}%", k.running_words))
                                    .on_hover_text(format!("Known from {}", k.source));
                            });
                            row.col(|ui| {
                                let off = style_off.get(e.id.as_str()).copied().unwrap_or(0.0);
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
                            });
                        }
                        None => {
                            row.col(|ui| {
                                ui.label(dash()).on_hover_text(
                                    "Nothing was marked known: Anki was off and no levels or word lists were ticked",
                                );
                            });
                            row.col(|ui| {
                                ui.label(dash());
                            });
                        }
                    }
                    if style_on {
                        row.col(|ui| match style_scores.get(e.id.as_str()) {
                            Some(&x) => {
                                ui.label(format!("{} · {}", style_scale.pct(x), style::label(x)))
                                    .on_hover_text(style_scale.hover(x));
                            }
                            None if s.language == Language::Chinese => {
                                ui.label(dash()).on_hover_text(
                                    style_skipped
                                        .get(&e.id)
                                        .map_or("Not rated yet", String::as_str),
                                );
                            }
                            None => {
                                ui.label(dash()).on_hover_text("Chinese books only");
                            }
                        });
                    }
                    row.col(|ui| {
                        let mins = minutes(s);
                        ui.label(format!(
                            "{}h {:02}m",
                            (mins / 60.0) as u64,
                            (mins % 60.0) as u64
                        ));
                    });
                    row.col(|ui| {
                        ui.label(history::age(e.analysed, now))
                            .on_hover_text(history::date_time(e.analysed));
                    });
                    row.col(|ui| match read_at.get(&e.id) {
                        Some(&t) => {
                            if ui
                                .button(RichText::new("✔").strong().color(palette::good()))
                                .on_hover_text(format!(
                                    "Marked read {} ({}). Click to change how many of its words count as known, or to unmark it.",
                                    history::age(t, now),
                                    history::date_time(t)
                                ))
                                .clicked()
                            {
                                mark_read = Some(e.id.clone());
                            }
                        }
                        None => {
                            let has_words = exists.get(&hist.profile(&e.id));
                            if ui
                                .add_enabled(has_words, egui::Button::new(RichText::new("✔").weak()))
                                .on_hover_text("Mark read: count its most frequent words as known and take it off the reading path")
                                .on_disabled_hover_text("No word data yet: open the book, or ⟳ Recalculate library")
                                .clicked()
                            {
                                mark_read = Some(e.id.clone());
                            }
                        }
                    });
                    row.col(|ui| {
                        // Apart from ✔, so as not to be hit instead of it.
                        ui.add_space(6.0);
                        if ui
                            .small_button("✖")
                            .on_hover_text(
                                "Remove from the library (the book itself is not touched); it can be put back for a few seconds",
                            )
                            .clicked()
                        {
                            remove = Some(e.id.clone());
                        }
                    });
                });
            });
        if let Some(s) = sort {
            self.history_sort = s;
        }
        if let Some(id) = mark_read {
            self.open_read_dialog(&id);
        }
        if let Some(id) = remove {
            self.remove_book(&id, ctx.input(|i| i.time));
        }
        if let Some(p) = open {
            self.open(p, ctx);
        }
    }

    /// Take a book out of the Library; its cover and word data are deleted
    /// once it can no longer be put back ([`UNDO_SECS`]).
    fn remove_book(&mut self, id: &str, now: f64) {
        self.finish_removal();
        match self.history.take(id) {
            Ok(Some((index, entry))) => {
                self.removed = Some(Removed {
                    index,
                    entry,
                    until: now + UNDO_SECS,
                });
            }
            Ok(None) => {}
            Err(e) => self.history_error = Some(e),
        }
        self.profiles_version += 1;
    }

    /// Put back the book just removed.
    fn undo_removal(&mut self) {
        let Some(r) = self.removed.take() else { return };
        if let Err(e) = self.history.restore(r.index, r.entry) {
            self.history_error = Some(e);
        }
        self.profiles_version += 1;
    }

    /// The book removed can no longer be put back: delete its cover and word data.
    fn finish_removal(&mut self) {
        let Some(r) = self.removed.take() else { return };
        let id = &r.entry.id;
        self.history.forget(id);
        if !self.history.entries.iter().any(|e| e.id == *id) {
            self.covers.remove(id);
            self.exists.set(self.history.profile(id), false);
        }
    }
}

/// A column header that sorts its table: underlined under the pointer, with
/// a hand. A click sorts by it (`first_desc`: largest first), again reverses.
fn sort_header<C: Copy + PartialEq>(
    ui: &mut egui::Ui,
    title: &str,
    hover: &str,
    c: C,
    (col, desc): (C, bool),
    first_desc: bool,
) -> Option<(C, bool)> {
    let arrow = match (col == c, desc) {
        (false, _) => "",
        (true, true) => " ⏷",
        (true, false) => " ⏶",
    };
    let r = ui
        .add(
            egui::Label::new(RichText::new(format!("{title}{arrow}")).strong())
                .sense(egui::Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if r.hovered() {
        let y = r.rect.bottom() - 1.0;
        let stroke = egui::Stroke::new(1.0, ui.visuals().strong_text_color());
        ui.painter().hline(r.rect.x_range(), y, stroke);
    }
    let hover = if hover.is_empty() {
        "Click to sort; again to reverse.".to_owned()
    } else {
        format!("{hover}\n\nClick to sort; again to reverse.")
    };
    r.on_hover_text(hover)
        .clicked()
        .then(|| (c, if col == c { !desc } else { first_desc }))
}

/// A small ⓘ button that shows an explanation while clicked open, for what
/// would otherwise only be in a tooltip (which needs a mouse, and hovering
/// at the right place).
fn info_button(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    let r = ui
        .add(
            egui::Button::new(RichText::new("ⓘ").weak())
                .frame(false)
                .small(),
        )
        .on_hover_text("Click for an explanation");
    egui::Popup::from_toggle_button_response(&r)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(440.0)
        .show(|ui| {
            ui.set_max_width(440.0);
            content(ui);
        });
    r
}

impl App {
    /// The library of the current language's books `have` (with a profile),
    /// folded as the known words need.
    fn lib_key(&self, have: Vec<String>) -> LibKey {
        LibKey {
            lang: self.language,
            fold: plan::Library::fold_for(
                self.known.merged.as_deref(),
                self.language,
                self.anki.settings.match_across_scripts,
            ),
            skip_hapax: self.settings.skip_hapax,
            ids: have,
            version: self.profiles_version,
        }
    }

    /// History ids of the current language's books that have a profile.
    fn plan_have(&mut self) -> Vec<String> {
        let key = (
            self.language,
            self.profiles_version,
            self.history.entries.len(),
        );
        if let Some((k, ids)) = &self.plan.have
            && *k == key
        {
            return ids.clone();
        }
        let ids: Vec<String> = self
            .history
            .entries
            .iter()
            .filter(|e| e.stats.language == self.language && self.history.profile(&e.id).is_file())
            .map(|e| e.id.clone())
            .collect();
        self.plan.have = Some((key, ids.clone()));
        ids
    }

    /// Plan in the background; a running plan is cancelled.
    fn start_plan(&mut self, inputs: PlanInputs, ctx: &egui::Context) {
        if let Some((_, c)) = self.plan.job.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.plan.inputs = Some(inputs.clone());
        if inputs.books.is_empty() {
            self.plan.result = None;
            return;
        }
        let cached = self
            .plan
            .lib
            .as_ref()
            .filter(|(k, _)| *k == inputs.lib)
            .map(|(_, l)| l.clone());
        let titles: HashMap<&str, &str> = self
            .history
            .entries
            .iter()
            .map(|e| (e.id.as_str(), e.title.as_str()))
            .collect();
        let files: Vec<(String, PathBuf)> = inputs
            .lib
            .ids
            .iter()
            .map(|id| {
                (
                    titles.get(id.as_str()).unwrap_or(&"?").to_string(),
                    self.history.profile(id),
                )
            })
            .collect();
        let k = self.known.merged.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let key = inputs.lib.clone();
            let lib = cached.unwrap_or_else(|| {
                let books: Vec<(String, Profile)> = files
                    .into_iter()
                    .filter_map(|(t, p)| Profile::load(&p).map(|p| (t, p)))
                    .collect();
                Arc::new(plan::Library::build(
                    key.lang,
                    key.fold,
                    key.skip_hapax,
                    books,
                ))
            });
            // Writing style: a copy only when the offsets changed.
            let mut lib = lib;
            let off: HashMap<&str, f64> = inputs
                .offsets
                .iter()
                .map(|(id, o)| (id.as_str(), *o))
                .collect();
            let want = |id: &str| off.get(id).copied().unwrap_or(0.0);
            if lib.books.iter().any(|b| b.offset != want(&b.id)) {
                Arc::make_mut(&mut lib).set_offsets(want);
            }
            let index: HashMap<&str, usize> = lib
                .books
                .iter()
                .enumerate()
                .map(|(i, b)| (b.id.as_str(), i))
                .collect();
            let books: Vec<usize> = inputs
                .books
                .iter()
                .filter_map(|id| index.get(id.as_str()).copied())
                .collect();
            let k0 = lib.known(k.as_deref());
            let params = inputs.params;
            let width = plan::width_for(&lib, &books, plan::SEARCH_BUDGET);
            let Some(best) = lib.optimise(&k0, &params, &books, width, &c2) else {
                return;
            };
            let naive = lib.evaluate(&k0, &params, &lib.easiest_now(&k0, &books));
            let (path, optimised) = match &inputs.manual {
                Some(order) => {
                    // The hand-set order, then books added since, as the planner placed them.
                    let mut o: Vec<usize> = order
                        .iter()
                        .filter_map(|id| index.get(id.as_str()).copied())
                        .filter(|b| books.contains(b))
                        .collect();
                    for s in &best.steps {
                        if !o.contains(&s.book) {
                            o.push(s.book);
                        }
                    }
                    // A volume added since, or an order saved before series
                    // were recognised, may be out of series order.
                    let o = lib.in_series_order(&o);
                    (lib.evaluate(&k0, &params, &o), Some(best))
                }
                None => (best, None),
            };
            let _ = tx.send(PlanDone {
                key,
                known_words: k0.len(),
                lib: lib.clone(),
                path,
                naive,
                optimised,
                secs: t.elapsed().as_secs_f64(),
            });
            ctx.request_repaint();
        });
        self.plan.job = Some((rx, cancel));
    }

    fn poll_plan(&mut self, ctx: &egui::Context) {
        let Some((rx, _)) = &self.plan.job else {
            return;
        };
        match rx.try_recv() {
            Ok(d) => {
                self.plan.lib = Some((d.key.clone(), d.lib.clone()));
                self.plan.result = Some(d);
                self.plan.job = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100))
            }
            Err(_) => self.plan.job = None,
        }
    }

    /// Open `files`: one is opened as usual; of several, all but the last
    /// are analysed in the background and recorded in the History, then the
    /// last is opened, so the book shown is the last one analysed.
    fn open_files(&mut self, mut files: Vec<PathBuf>, ctx: &egui::Context) {
        let Some(last) = files.pop() else { return };
        if files.is_empty() {
            self.open(last, ctx);
            return;
        }
        if self
            .batch
            .as_ref()
            .is_some_and(|b| b.mode != BatchMode::Auto)
        {
            self.toast = Some((
                "Still analysing the books opened before: open these once it has finished.".into(),
                ctx.input(|i| i.time) + 4.0,
            ));
            return;
        }
        let todo = files
            .into_iter()
            .map(|p| self.batch_item(file_name(&p), p, self.segmenter))
            .collect();
        self.start_batch(todo, Some(last), BatchMode::Open, ctx);
    }

    /// A book for a batch, with the sections saved for it in the Library.
    fn batch_item(&self, label: String, path: PathBuf, kind: SegmenterKind) -> BatchItem {
        let abs = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
        let sections = self
            .history
            .entries
            .iter()
            .find(|e| e.path == abs)
            .and_then(|e| e.sections.clone());
        BatchItem {
            label,
            path,
            kind,
            sections,
            refresh: None,
        }
    }

    /// A Library book for a batch, with the segmenter and sections it was
    /// last analysed with.
    fn library_item(e: &history::Entry) -> BatchItem {
        BatchItem {
            label: e.title.clone(),
            path: e.path.clone(),
            kind: segmenter_by_label(&e.stats.segmenter),
            sections: e.sections.clone(),
            refresh: None,
        }
    }

    /// A Library book whose counts are current, for its known words alone.
    fn coverage_item(e: &history::Entry) -> BatchItem {
        BatchItem {
            refresh: Some(Box::new(e.clone())),
            ..Self::library_item(e)
        }
    }

    /// Analyse every Library book whose file is still there again, so its row
    /// reflects the current known words, dictionary and settings.
    fn recalculate_library(&mut self, ctx: &egui::Context) {
        let todo: Vec<BatchItem> = self
            .history
            .entries
            .iter()
            .filter(|e| e.path.is_file())
            .map(Self::library_item)
            .collect();
        if todo.is_empty() {
            return;
        }
        self.history_error = None;
        self.start_batch(todo, None, BatchMode::Refresh, ctx);
    }

    /// What a Library row in `lang` is computed from besides its book, its
    /// sections and its segmenter: the build, the user dictionary, the known
    /// words (for the segmenter and for coverage) and the settings, as a
    /// digest. `None` while `lang`'s data is not loaded this session or the
    /// Anki cards are still being read, when a row could not be computed right.
    fn lang_inputs(&mut self, lang: Language) -> Option<Inputs> {
        use sha2::{Digest, Sha256};
        let (user_words, anki, known) = if lang == self.language {
            (&self.user_words, &self.anki, &self.known)
        } else {
            let d = self.stash.get(&lang)?;
            (&d.user_words, &d.anki, &d.known)
        };
        let anki_pending = anki.rx.is_some()
            || anki.dirty
            || (anki.settings.enabled
                && anki.settings.collection.is_some()
                && anki.knowledge.is_none());
        if anki_pending {
            return None;
        }
        // Character folding changes a Chinese book's character counts.
        let fold = lang == Language::Chinese && anki.settings.match_across_scripts;
        // Asked every second: made again only when what it is made from changed.
        let quick = {
            use std::hash::{Hash, Hasher};
            let mut h = std::hash::DefaultHasher::new();
            user_words.hash(&mut h);
            for l in &known.settings.lists {
                l.words.hash(&mut h);
            }
            (self.settings.skip_hapax, fold).hash(&mut h);
            h.finish()
        };
        if let Some((q, m, i)) = self.inputs_memo.get(&lang)
            && *q == quick
            && match (m, &known.merged) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
        {
            return Some(i.clone());
        }
        let knowledge = match &known.merged {
            None => String::new(),
            Some(k) => match self.auto.knowledge.iter().find(|(a, _)| Arc::ptr_eq(a, k)) {
                Some((_, d)) => d.clone(),
                None => {
                    let d = knowledge_digest(k);
                    self.auto.knowledge.push((k.clone(), d.clone()));
                    // Only the recent ones: the others are no longer in use.
                    if self.auto.knowledge.len() > 8 {
                        self.auto.knowledge.remove(0);
                    }
                    d
                }
            },
        };
        let text = |h: &mut Sha256, s: &str| {
            h.update((s.len() as u64).to_le_bytes());
            h.update(s.as_bytes());
        };
        let hex = |h: Sha256| -> String {
            h.finalize()[..12]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        let mut h = Sha256::new();
        text(&mut h, env!("HANSTAT_LIBRARY_FINGERPRINT"));
        text(&mut h, &format!("{lang:?}"));
        h.update([u8::from(fold)]);
        h.update((user_words.len() as u64).to_le_bytes());
        for w in user_words {
            text(&mut h, w);
        }
        let seg = known.settings.segmenter_words();
        h.update((seg.len() as u64).to_le_bytes());
        for w in &seg {
            text(&mut h, w);
        }
        let analysis = hex(h);
        let mut h = Sha256::new();
        h.update([u8::from(self.settings.skip_hapax)]);
        text(&mut h, &knowledge);
        let inputs = Inputs {
            analysis,
            coverage: hex(h),
        };
        self.inputs_memo
            .insert(lang, (quick, known.merged.clone(), inputs.clone()));
        Some(inputs)
    }

    /// Analyse again, in the background, the Library books whose rows were
    /// computed from other inputs ([`Self::lang_inputs`]) than the current
    /// ones, once these have not changed for [`AUTO_REFRESH_DELAY`]. A change
    /// while it runs stops it, and it starts afresh after the delay. The open
    /// book's row is kept up to date by the Overview itself.
    fn auto_refresh(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if now < self.auto.next_scan {
            return;
        }
        self.auto.next_scan = now + 1.0;
        // As start_batch takes them, so the two compare.
        let mut inputs = HashMap::new();
        for l in Language::ALL {
            if let Some(d) = self.lang_inputs(l) {
                inputs.insert(l, d);
            }
        }
        if inputs != self.auto.seen {
            self.auto.seen = inputs.clone();
            self.auto.changed_at = now;
            self.auto.failed.clear();
        }
        if self
            .batch
            .as_ref()
            .is_some_and(|b| b.mode == BatchMode::Auto && b.inputs != inputs)
        {
            // Outdated: what it has finished stays, the rest waits for the delay.
            let b = self.batch.take().unwrap();
            b.cancel.store(true, Ordering::Relaxed);
        }
        if self.batch.is_some() || self.job.is_some() || inputs.is_empty() {
            return;
        }
        let wait = self.auto.changed_at + AUTO_REFRESH_DELAY - now;
        if wait > 0.0 {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.min(1.0)));
            return;
        }
        if self.auto.paused.as_ref() == Some(&inputs) {
            return;
        }
        self.auto.paused = None;
        let open = self.book_id.as_deref();
        // Rows of which only the known words changed first: they take
        // milliseconds each, the others an analysis.
        let (mut todo, mut full) = (Vec::new(), Vec::new());
        for e in &self.history.entries {
            let Some(d) = inputs.get(&e.stats.language) else {
                continue;
            };
            if e.inputs == d.joined()
                || Some(e.id.as_str()) == open
                || self.auto.failed.get(&e.path) == Some(d)
            {
                continue;
            }
            if Inputs::analysis_of(&e.inputs) == Some(d.analysis.as_str()) {
                todo.push(Self::coverage_item(e));
            } else if e.path.is_file() {
                full.push(Self::library_item(e));
            } else {
                self.auto.failed.insert(e.path.clone(), d.clone());
            }
        }
        todo.append(&mut full);
        if !todo.is_empty() {
            self.start_batch(todo, None, BatchMode::Auto, ctx);
        }
    }

    /// Analyse `todo` one after another in the background, the way opening
    /// each does, and record each in the History with its word data and
    /// cover. Then open `then_open`, if given and not stopped. A batch
    /// already running is stopped after its current book.
    fn start_batch(
        &mut self,
        todo: Vec<BatchItem>,
        then_open: Option<PathBuf>,
        mode: BatchMode,
        ctx: &egui::Context,
    ) {
        if let Some(b) = self.batch.take() {
            b.cancel.store(true, Ordering::Relaxed);
        }
        // Each language's dictionary, known words for the segmenter, and
        // knowledge for the History's "Known" (languages not used yet: none).
        let mut words: HashMap<Language, Vec<String>> = self
            .stash
            .iter()
            .map(|(l, d)| (*l, d.user_words.clone()))
            .collect();
        words.insert(self.language, self.user_words.clone());
        let mut known_words: HashMap<Language, Vec<String>> = self
            .stash
            .iter()
            .map(|(l, d)| (*l, d.known.settings.segmenter_words()))
            .collect();
        known_words.insert(self.language, self.known.settings.segmenter_words());
        let mut knowledge: HashMap<Language, Arc<anki::Knowledge>> = self
            .stash
            .iter()
            .filter_map(|(l, d)| d.known.merged.clone().map(|k| (*l, k)))
            .collect();
        if let Some(k) = &self.known.merged {
            knowledge.insert(self.language, k.clone());
        }
        let mut inputs = HashMap::new();
        for l in Language::ALL {
            if let Some(d) = self.lang_inputs(l) {
                inputs.insert(l, d);
            }
        }
        let thread_inputs = inputs.clone();
        let cache = self.cache.clone();
        let skip_hapax = self.settings.skip_hapax;
        let fold_chars = self.fold_chars();
        let history_path = History::path();
        let (tx, rx) = channel();
        let done = Arc::new(AtomicUsize::new(0));
        let current = Arc::new(Mutex::new(String::new()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (d2, cur2, c2) = (done.clone(), current.clone(), cancel.clone());
        let total = todo.len() + usize::from(then_open.is_some());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for item in todo {
                if c2.load(Ordering::Relaxed) {
                    break;
                }
                let BatchItem {
                    label,
                    path,
                    kind,
                    sections,
                    refresh,
                } = item;
                *cur2.lock().unwrap() = label.clone();
                ctx.request_repaint();
                if let Some(e) = refresh.and_then(|e| {
                    coverage_only(&e, &knowledge, skip_hapax, &history_path, &thread_inputs)
                }) {
                    let _ = tx.send(BatchMsg::Book(Box::new(e), false));
                    d2.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let res = profile::analyse_file(
                    &path,
                    kind,
                    sections.as_deref(),
                    &|l| {
                        words
                            .get(&l)
                            .cloned()
                            .unwrap_or_else(|| hanstat::load_user_dict(l).0)
                    },
                    &|l| {
                        known_words
                            .get(&l)
                            .cloned()
                            .unwrap_or_else(|| known::Settings::load_for(l).0.segmenter_words())
                    },
                    cache.as_ref(),
                    fold_chars,
                )
                .and_then(|a| {
                    let id = a
                        .file_id
                        .ok_or_else(|| format!("cannot read {}", path.display()))?;
                    let mut r = a.report;
                    let names: HashSet<String> = a.names.into_iter().collect();
                    if let Some(k) = knowledge.get(&r.language) {
                        r.anki = Some(BookCoverage::compute(
                            k,
                            &r.char_freq,
                            &r.word_freq,
                            &names,
                            &r.word_parts,
                            skip_hapax,
                        ));
                    }
                    if let Some(cover) = &a.book.cover {
                        let dest = history::cover_path(&id);
                        if !dest.is_file() {
                            let _ = history::save_cover(cover, &dest);
                        }
                    }
                    // Written only when it changed, so the reading path is
                    // not rebuilt for nothing.
                    let p = Profile::from_report(&id, &r, &names);
                    let p_path = profile::path_in(&history_path, &id);
                    let changed = Profile::load(&p_path).map(|o| o.digest()) != Some(p.digest());
                    if changed {
                        p.save(&p_path)?;
                    }
                    Ok((
                        history::Entry {
                            id,
                            path: std::path::absolute(&path).unwrap_or_else(|_| path.clone()),
                            title: r
                                .title
                                .clone()
                                .filter(|t| !t.trim().is_empty())
                                .unwrap_or_else(|| file_stem(&path)),
                            analysed: history::now(),
                            sections: Some(r.sections.iter().map(|s| s.included).collect()),
                            inputs: thread_inputs
                                .get(&r.language)
                                .map(Inputs::joined)
                                .unwrap_or_default(),
                            stats: history::Stats::from_report(&r),
                        },
                        changed,
                    ))
                });
                let _ = tx.send(match res {
                    Ok((e, changed)) => BatchMsg::Book(Box::new(e), changed),
                    Err(e) => BatchMsg::Failed(label, path, e),
                });
                d2.fetch_add(1, Ordering::Relaxed);
                ctx.request_repaint();
            }
            let _ = tx.send(BatchMsg::Finished);
            ctx.request_repaint();
        });
        self.batch = Some(Batch {
            mode,
            inputs,
            rx,
            done,
            total,
            current,
            cancel,
            failed: Vec::new(),
            then_open,
        });
    }

    fn poll_batch(&mut self, ctx: &egui::Context) {
        let Some(b) = &mut self.batch else {
            return;
        };
        let mut finished = false;
        while let Ok(m) = b.rx.try_recv() {
            match m {
                BatchMsg::Book(e, changed) => {
                    self.exists.set(self.history.profile(&e.id), true);
                    let res = match b.mode {
                        BatchMode::Open => self.history.record(*e, true),
                        BatchMode::Refresh | BatchMode::Auto => self.history.refresh(*e),
                    };
                    if let Err(e) = res {
                        self.history_error = Some(e);
                    }
                    if changed {
                        self.profiles_version += 1;
                    }
                }
                BatchMsg::Failed(label, path, e) => {
                    if b.mode == BatchMode::Auto
                        && let Some(d) = self
                            .history
                            .entries
                            .iter()
                            .find(|x| x.path == path)
                            .and_then(|x| b.inputs.get(&x.stats.language))
                            .cloned()
                    {
                        self.auto.failed.insert(path, d);
                    }
                    b.failed.push(format!("{label}: {e}"));
                }
                BatchMsg::Finished => finished = true,
            }
        }
        if !finished {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
            return;
        }
        let b = self.batch.take().unwrap();
        let stopped = b.cancel.load(Ordering::Relaxed);
        if b.mode == BatchMode::Auto && stopped {
            // Stopped by the user: not again until something changes.
            self.auto.paused = Some(b.inputs.clone());
        }
        if let Some(p) = b.then_open
            && !stopped
        {
            self.open(p, ctx);
        }
        // After opening, which clears the error line.
        if !b.failed.is_empty() {
            let what = b.failed.join("; ");
            match b.mode {
                BatchMode::Open => self.set_error("batch", format!("Could not analyse {what}")),
                BatchMode::Refresh | BatchMode::Auto => {
                    self.history_error = Some(format!("Could not update {what}"))
                }
            }
        }
    }

    /// The batch's progress, with a button to stop it.
    fn batch_status(&self, ui: &mut egui::Ui) {
        let Some(b) = &self.batch else { return };
        ui.spinner();
        let done = b.done.load(Ordering::Relaxed);
        let doing = match b.mode {
            BatchMode::Open => "Analysing",
            BatchMode::Refresh => "Recalculating the Library:",
            BatchMode::Auto => "Updating the Library:",
        };
        let r = ui.label(format!(
            "{doing} {} of {}: {}",
            (done + 1).min(b.total),
            b.total,
            b.current.lock().unwrap()
        ));
        if b.mode == BatchMode::Auto {
            r.on_hover_text(
                "Known words, the user dictionary, a setting or the analysis itself changed since these books were analysed: \
                 their rows are being updated",
            );
        }
        if ui
            .small_button("Stop")
            .on_hover_text(match b.mode {
                BatchMode::Auto => {
                    "Stop after the current book; the rows updated so far stay. \
                     It starts again only when something changes"
                }
                _ => "Stop after the current book; the books analysed so far stay in the Library",
            })
            .clicked()
        {
            b.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The reading path of the History's books in the current language.
    fn plan_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let lang = self.language;
        let fixed_lang = self.prep.is_some() || self.job.is_some();
        let mut pick_lang = None;
        let mut changed = false;
        let style_on = self.style_on() && lang == Language::Chinese;
        let st = &mut self.plan.settings;
        ui.horizontal_wrapped(|ui| {
            if fixed_lang {
                ui.label(RichText::new(format!("{} books", lang.name())).strong())
                    .on_hover_text("The open book's language. Close it (or open another) to plan another language.");
            } else {
                egui::ComboBox::from_id_salt("plan_lang")
                    .selected_text(format!("{} books", lang.name()))
                    .show_ui(ui, |ui| {
                        for l in Language::ALL {
                            if ui.selectable_label(l == lang, l.name()).clicked() {
                                pick_lang = Some(l);
                            }
                        }
                    });
            }
            ui.separator();
            ui.label("Before each book, study");
            changed |= ui.radio_value(&mut st.fixed, false, "until").changed();
            changed |= ui
                .add_enabled(
                    !st.fixed,
                    egui::DragValue::new(&mut st.target)
                        .range(50.0..=100.0)
                        .speed(0.1)
                        .max_decimals(1)
                        .suffix("%"),
                )
                .on_hover_text("95%: adequate comprehension; 98%: comfortable reading (L2 lexical-coverage research, as on the Overview)")
                .changed();
            ui.label("of its running words are known, or");
            changed |= ui.radio_value(&mut st.fixed, true, "").changed();
            changed |= ui
                .add_enabled(
                    st.fixed,
                    egui::DragValue::new(&mut st.learn).range(0..=20_000).speed(5.0),
                )
                .changed();
            ui.label("of its words");
            ui.separator();
            changed |= ui
                .checkbox(&mut st.incidental_on, "Words read")
                .on_hover_text(
                    "Count an unknown word read this many times in a book as learned from reading it. \
                     Off by default: how many encounters it takes varies widely, and words learned this way are often forgotten.",
                )
                .changed();
            changed |= ui
                .add_enabled(
                    st.incidental_on,
                    egui::DragValue::new(&mut st.incidental).range(2..=1000).suffix("+ times"),
                )
                .changed();
            ui.label("are learned");
            ui.separator();
            ui.label("Look ahead").on_hover_text(
                "How much the books still to come count when choosing which words to study: \
                 0 studies each book's most frequent unknown words (the fewest for that book); \
                 higher prefers words the rest of the path needs too.",
            );
            changed |= ui
                .add(
                    egui::DragValue::new(&mut st.lookahead)
                        .range(0.0..=5.0)
                        .speed(0.01)
                        .max_decimals(2),
                )
                .changed();
        });
        if changed && let Err(e) = st.save() {
            self.plan.error = Some(e);
        }
        if let Some(l) = pick_lang {
            self.set_language(l, ctx);
            return;
        }
        if let Some(e) = &self.plan.error {
            ui.colored_label(palette::warn(), format!("⚠ {e}"));
        }

        let have = self.plan_have();
        let have_set: HashSet<&str> = have.iter().map(String::as_str).collect();
        let missing: Vec<(String, PathBuf)> = self
            .history
            .entries
            .iter()
            .filter(|e| e.stats.language == lang && !have_set.contains(e.id.as_str()))
            .map(|e| (e.title.clone(), e.path.clone()))
            .collect();
        let lib = self.lib_key(have.clone());
        let st = &self.plan.settings;
        let offsets: Vec<(String, f64)> = if style_on {
            have.iter()
                .map(|id| (id.clone(), self.style_offset(id)))
                .filter(|(_, o)| *o != 0.0)
                .collect()
        } else {
            Vec::new()
        };
        let k = self.known.merged.clone();
        let inputs = PlanInputs {
            lib,
            knowledge: k.as_ref().map_or(0, |k| Arc::as_ptr(k) as usize),
            params: st.params(),
            // Books marked read are done with.
            books: have
                .iter()
                .filter(|id| {
                    !st.left_out.contains(id) && self.known.settings.read_book(id).is_none()
                })
                .cloned()
                .collect(),
            manual: st.manual_order(lang).cloned(),
            offsets,
        };
        if self.plan.inputs.as_ref() != Some(&inputs) {
            self.start_plan(inputs, ctx);
        }
        let entries: Vec<&history::Entry> = self
            .history
            .entries
            .iter()
            .filter(|e| e.stats.language == lang)
            .collect();

        let mut act: Option<PlanAction> = None;
        // Books without word data.
        if self.batch.is_some() {
            ui.horizontal(|ui| self.batch_status(ui));
        } else if !missing.is_empty() {
            let can = missing.iter().filter(|(_, p)| self.exists.get(p)).count();
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "{} of the {} {} books have no word data yet (analysed by an older version) and are not on the path.",
                    missing.len(),
                    entries.len(),
                    lang.name()
                ));
                if can > 0
                    && ui
                        .button(format!("Analyse {can}"))
                        .on_hover_text("Analyse them again in the background, with their default sections and the current segmenter")
                        .clicked()
                {
                    act = Some(PlanAction::Backfill);
                }
                if can < missing.len() {
                    ui.label(
                        RichText::new(format!("{} are no longer where they were.", missing.len() - can))
                            .weak(),
                    );
                }
            });
        }

        let Some(res) = &self.plan.result else {
            ui.add_space(20.0);
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(if self.plan.job.is_some() {
                        "Planning…"
                    } else if entries.is_empty() {
                        "No books in this language yet. Open or drop an .epub or .txt file."
                    } else {
                        "No books on the path."
                    })
                    .size(20.0)
                    .weak(),
                );
            });
            self.apply_plan_action(act, ui, ctx);
            return;
        };
        let lib = &res.lib;
        let path = &res.path;
        let fixed = matches!(path.params.study, plan::Study::Fixed(_));
        let source = k
            .as_ref()
            .map(|k| match (k.from_anki, k.listed.source.is_empty()) {
                (true, true) => "Anki".to_owned(),
                (true, false) => format!("Anki + {}", k.listed.source),
                (false, _) => k.listed.source.clone(),
            })
            .unwrap_or_else(|| "nothing marked known".to_owned());
        ui.horizontal_wrapped(|ui| {
            if self.plan.job.is_some() {
                ui.spinner();
            }
            ui.label(format!(
                "{} books · you know {} of their {} distinct words ({source})",
                path.steps.len(),
                fmt_int(res.known_words as u64),
                fmt_int(lib.vocab_len() as u64),
            ));
            ui.label(RichText::new(format!("· planned in {:.1} s", res.secs)).weak());
        });
        let summary = |p: &plan::Path| {
            if fixed {
                format!(
                    "lowest coverage while reading {:.1}%, average {:.1}%",
                    p.lowest_reading(),
                    p.steps.iter().map(|s| s.reading).sum::<f64>() / p.steps.len().max(1) as f64
                )
            } else {
                format!(
                    "{} words to study in all, at most {} before one book",
                    fmt_int(p.total_study() as u64),
                    fmt_int(p.peak() as u64)
                )
            }
        };
        ui.horizontal_wrapped(|ui| {
            match &res.optimised {
                Some(best) => {
                    ui.label(RichText::new(format!("Your order: {}.", summary(path))).strong());
                    ui.label(format!("The planner's order: {}.", summary(best)));
                    if ui.button("Use the planner's order").clicked() {
                        act = Some(PlanAction::Optimise);
                    }
                }
                None => {
                    ui.label(RichText::new(format!("This order: {}.", summary(path))).strong());
                }
            }
            ui.label(
                RichText::new(format!(
                    "Reading the easiest book first, then the next: {}.",
                    summary(&res.naive)
                ))
                .weak(),
            );
        });
        let books: Vec<usize> = path.order();
        let segs = lib.segmenters(&books);
        if segs.len() > 1 {
            ui.label(
                RichText::new(format!(
                    "The books were segmented with {}: words they share are undercounted where the segmenters split them differently.",
                    segs.join(", ")
                ))
                .weak(),
            );
        }
        ui.add_space(4.0);

        let by_id: HashMap<&str, &history::Entry> =
            entries.iter().map(|e| (e.id.as_str(), *e)).collect();
        let style_scores: HashMap<&str, f32> = if style_on {
            self.style.store.current()
        } else {
            HashMap::new()
        };
        let style_scale = self.style_scale();
        let exists = &self.exists;
        let covers = &mut self.covers;
        let hist = &self.history;
        let n = path.steps.len();
        let query = search::Query::new(&self.library_filter);
        // The steps shown: all, or those the search matches (numbered as on the whole path).
        let shown: Vec<usize> = (0..n)
            .filter(|&i| {
                let b = &lib.books[path.steps[i].book];
                query.is_empty()
                    || match by_id.get(b.id.as_str()) {
                        Some(e) => book_matches(&query, &b.title, &e.path),
                        None => query.matches([b.title.as_str()]),
                    }
            })
            .collect();
        if shown.is_empty() {
            ui.label(RichText::new("No book on the path matches the search.").weak());
        }
        let dash = || RichText::new("–").weak();
        let left_out: Vec<(String, String)> = self
            .plan
            .settings
            .left_out
            .iter()
            .filter_map(|id| {
                by_id
                    .get(id.as_str())
                    .map(|e| (id.clone(), e.title.clone()))
            })
            .collect();
        let read_here = self
            .known
            .settings
            .read
            .iter()
            .filter(|b| by_id.contains_key(b.id.as_str()))
            .count();
        let table_h = (ui.available_height()
            - if left_out.is_empty() && read_here == 0 {
                0.0
            } else {
                72.0
            })
        .max(120.0);
        TableBuilder::new(ui)
            .id_salt("plan_table")
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .striped(true)
            .max_scroll_height(table_h)
            .column(Column::exact(88.0))
            .column(
                // Narrower while the writing-style column is shown, so the row still fits.
                Column::initial(if style_on { 200.0 } else { 300.0 })
                    .at_least(140.0)
                    .resizable(true)
                    .clip(true),
            )
            .columns(Column::exact(130.0), style_on as usize)
            .columns(Column::exact(96.0), 6)
            .column(Column::remainder())
            .header(24.0, |mut h| {
                let mut heads: Vec<(&str, &str)> = vec![
                    ("#", "Reading order. ⏶⏷ set your own order; the planner's is shown alongside"),
                    ("Book", "Click a title to open the book"),
                    ("Known now", "Running words you know now (names left out)"),
                    ("When reached", "Running words known when the path reaches the book, before studying for it"),
                    (
                        "Study",
                        "Words to study before reading it",
                    ),
                    (
                        if fixed { "" } else { "If read now" },
                        "Words it would take to reach the target if you read it next",
                    ),
                    ("From earlier", "Words of this book you don't know now that the path teaches before it"),
                    ("While reading", "Running words known while reading it, after studying"),
                    ("", ""),
                ];
                if style_on {
                    heads.insert(
                        2,
                        (
                            "Writing style",
                            "How hard the prose is when you know its words (⚙); a book above typical gets a higher target, one below a lower",
                        ),
                    );
                }
                for (t, hover) in heads {
                    h.col(|ui| {
                        ui.strong(t).on_hover_text(hover);
                    });
                }
            })
            .body(|body| {
                body.rows(COVER_ROW, shown.len(), |mut row| {
                    let i = shown[row.index()];
                    let s = &path.steps[i];
                    let b = &lib.books[s.book];
                    let e = by_id.get(b.id.as_str());
                    row.col(|ui| {
                        ui.label(RichText::new(format!("{}", i + 1)).strong());
                        // A series is read in volume order.
                        let after_volume = i > 0 && lib.precedes(path.steps[i - 1].book, s.book);
                        let before_volume = i + 1 < n && lib.precedes(s.book, path.steps[i + 1].book);
                        if ui
                            .add_enabled(i > 0 && !after_volume, egui::Button::new("⏶").small())
                            .on_hover_text("Read earlier")
                            .on_disabled_hover_text(if after_volume { "The book above is an earlier volume of its series" } else { "" })
                            .clicked()
                        {
                            act = Some(PlanAction::Move(i, -1));
                        }
                        if ui
                            .add_enabled(i + 1 < n && !before_volume, egui::Button::new("⏷").small())
                            .on_hover_text("Read later")
                            .on_disabled_hover_text(if before_volume { "The book below is a later volume of its series" } else { "" })
                            .clicked()
                        {
                            act = Some(PlanAction::Move(i, 1));
                        }
                    });
                    row.col(|ui| {
                        let tex = covers
                            .entry(b.id.clone())
                            .or_insert_with(|| load_cover(ctx, &hist.cover(&b.id)));
                        let (w, h) = COVER_SIZE;
                        match tex {
                            Some(t) => {
                                let [tw, th] = t.size().map(|x| x as f32);
                                let k = (w / tw).min(h / th);
                                ui.allocate_ui(egui::vec2(w, h), |ui| {
                                    ui.centered_and_justified(|ui| {
                                        ui.add(
                                            egui::Image::new(&*t)
                                                .fit_to_exact_size(egui::vec2(tw * k, th * k))
                                                .corner_radius(2.0),
                                        );
                                    });
                                });
                            }
                            None => {
                                ui.allocate_space(egui::vec2(w, h));
                            }
                        }
                        ui.add_space(6.0);
                        let hover = format!(
                            "{}\n{} · {} distinct words, {} running",
                            e.map(|e| e.path.display().to_string()).unwrap_or_default(),
                            b.segmenter,
                            fmt_int(b.unique_words() as u64),
                            fmt_int(b.running_words()),
                        );
                        match e.filter(|e| exists.get(&e.path)) {
                            Some(e) => {
                                if ui.link(&b.title).on_hover_text(hover).clicked() {
                                    act = Some(PlanAction::Open(e.path.clone()));
                                }
                            }
                            None => {
                                ui.label(RichText::new(&b.title).weak()).on_hover_text(hover);
                            }
                        }
                    });
                    if style_on {
                        row.col(|ui| match style_scores.get(b.id.as_str()) {
                            Some(&x) => {
                                let t = ui.label(format!("{} · {}", style_scale.pct(x), style::label(x)));
                                let mut hover = style_scale.hover(x);
                                if s.offset.abs() >= 0.05 {
                                    hover.push_str("\n\n");
                                    hover.push_str(&match path.params.study {
                                        plan::Study::Target(target) => format!(
                                            "Target {:.1}% ({:+.1} points for its writing style)",
                                            (target + s.offset).clamp(0.0, 100.0),
                                            s.offset
                                        ),
                                        plan::Study::Fixed(_) => format!(
                                            "Counts as read at {:.1}% ({:+.1} points for its writing style)",
                                            s.reading - s.offset,
                                            -s.offset
                                        ),
                                    });
                                }
                                t.on_hover_text(hover);
                            }
                            None => {
                                ui.label(dash()).on_hover_text("Not rated yet");
                            }
                        });
                    }
                    row.col(|ui| {
                        ui.label(format!("{:.1}%", s.now));
                    });
                    row.col(|ui| {
                        let up = s.reached - s.now;
                        let t = ui.label(format!("{:.1}%", s.reached));
                        if up >= 0.05 {
                            t.on_hover_text(format!("+{up:.1} points from the books before it"));
                        }
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(fmt_int(s.study.len() as u64)).strong());
                    });
                    row.col(|ui| match s.study_now {
                        Some(x) => {
                            ui.label(fmt_int(x as u64));
                        }
                        None => {
                            ui.label(dash());
                        }
                    });
                    row.col(|ui| {
                        let t = ui.label(fmt_int(s.gained as u64));
                        if s.by_reading > 0 {
                            t.on_hover_text(format!(
                                "Reading it teaches {} more (read {}+ times)",
                                fmt_int(s.by_reading as u64),
                                path.params.incidental.unwrap_or(0)
                            ));
                        }
                    });
                    row.col(|ui| {
                        // Coloured by the difficulty, writing style included.
                        let d = Difficulty::from_percent(s.reading - s.offset);
                        ui.label(RichText::new(format!("{:.1}%", s.reading)).color(difficulty_color(d)))
                            .on_hover_text(format!("{}: {}", d.label(), d.description()));
                    });
                    row.col(|ui| {
                        if ui
                            .add_enabled(!s.study.is_empty(), egui::Button::new("💾").small())
                            .on_hover_text("Save the words to study before it (word, tab, count in the book): import it as a word list once learned, or make Anki cards from it")
                            .clicked()
                        {
                            act = Some(PlanAction::SaveList(i));
                        }
                        if ui
                            .small_button("✔")
                            .on_hover_text("Mark it read: its most frequent words count as known, and it leaves the path")
                            .clicked()
                        {
                            act = Some(PlanAction::MarkRead(b.id.clone()));
                        }
                        if ui
                            .small_button("✖")
                            .on_hover_text("Leave it out of the path without marking it read")
                            .clicked()
                        {
                            act = Some(PlanAction::LeaveOut(b.id.clone()));
                        }
                    });
                });
            });
        if !left_out.is_empty() || read_here > 0 {
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if read_here > 0 {
                    ui.label(
                        RichText::new(format!(
                            "{read_here} {} marked read {} not on the path.",
                            if read_here == 1 { "book" } else { "books" },
                            if read_here == 1 { "is" } else { "are" },
                        ))
                        .weak(),
                    )
                    .on_hover_text("Unmark one with its ✔ in the Library's Books tab");
                }
                if left_out.is_empty() {
                    return;
                }
                ui.label(RichText::new(format!("Left out ({}):", left_out.len())).weak());
                for (id, title) in &left_out {
                    if ui
                        .small_button(format!("↩ {title}"))
                        .on_hover_text("Put it back on the path")
                        .clicked()
                    {
                        act = Some(PlanAction::Include(id.clone()));
                    }
                }
            });
        }
        self.apply_plan_action(act, ui, ctx);
    }

    fn apply_plan_action(&mut self, act: Option<PlanAction>, ui: &egui::Ui, ctx: &egui::Context) {
        let Some(act) = act else { return };
        let lang = self.language.name().to_owned();
        let st = &mut self.plan.settings;
        let mut save_settings = true;
        match act {
            PlanAction::Open(p) => {
                save_settings = false;
                self.open(p, ctx);
            }
            PlanAction::Move(i, d) => {
                let Some(res) = &self.plan.result else { return };
                let mut ids: Vec<String> = res
                    .path
                    .steps
                    .iter()
                    .map(|s| res.lib.books[s.book].id.clone())
                    .collect();
                let j = (i as isize + d) as usize;
                ids.swap(i, j);
                st.manual.insert(lang, ids);
            }
            PlanAction::LeaveOut(id) => {
                if !st.left_out.contains(&id) {
                    st.left_out.push(id);
                }
            }
            PlanAction::Include(id) => st.left_out.retain(|x| *x != id),
            PlanAction::MarkRead(id) => {
                save_settings = false;
                self.open_read_dialog(&id);
            }
            PlanAction::Optimise => {
                st.manual.remove(&lang);
            }
            PlanAction::SaveList(i) => {
                save_settings = false;
                let Some(res) = &self.plan.result else { return };
                let s = &res.path.steps[i];
                let title = &res.lib.books[s.book].title;
                let mut text = format!(
                    "# Words to study before {title} (step {} of the reading path), with their counts in the book\n",
                    i + 1
                );
                for (w, n) in res.lib.study_list(s.book, &s.study) {
                    text.push_str(&format!("{w}\t{n}\n"));
                }
                let name: String = title
                    .chars()
                    .map(|c| {
                        if r#"<>:"/\|?*"#.contains(c) || c.is_control() {
                            '_'
                        } else {
                            c
                        }
                    })
                    .collect();
                if let Some(m) = save(&format!("{} - study list.txt", name.trim()), "txt", text) {
                    self.toast = Some((m, ui.input(|i| i.time) + 3.0));
                }
            }
            PlanAction::Backfill => {
                save_settings = false;
                let have = self.plan_have();
                let todo: Vec<BatchItem> = self
                    .history
                    .entries
                    .iter()
                    .filter(|e| e.stats.language == self.language && !have.contains(&e.id))
                    .filter(|e| e.path.is_file())
                    .map(Self::library_item)
                    .collect();
                self.plan.error = None;
                self.start_batch(todo, None, BatchMode::Refresh, ctx);
            }
        }
        if save_settings && let Err(e) = self.plan.settings.save() {
            self.plan.error = Some(e);
        }
    }
}

/// A book's reading speed: `speed` where changed from `default` (the ⚙
/// menu's), with a button to go back to it.
fn speed_input(
    ui: &mut egui::Ui,
    speed: &mut Option<f32>,
    default: f32,
    range: std::ops::RangeInclusive<f32>,
    step: f64,
    suffix: &str,
) {
    let mut v = speed.unwrap_or(default);
    if ui
        .add(
            egui::DragValue::new(&mut v)
                .range(range)
                .speed(step)
                .suffix(suffix),
        )
        .on_hover_text("This book only, to try another speed; the default is set in Settings (⚙)")
        .changed()
    {
        *speed = Some(v);
    }
    if speed.is_some_and(|s| s != default)
        && ui
            .small_button("↺")
            .on_hover_text(format!(
                "Back to the default, {default:.0}{suffix} (Settings, ⚙)"
            ))
            .clicked()
    {
        *speed = None;
    }
}

/// Library row `e` brought up to date from its saved word data, when only
/// the known words (or the hapax setting) changed; `None` if the word data
/// lacks what that needs (written before character counts were kept).
fn coverage_only(
    e: &history::Entry,
    knowledge: &HashMap<Language, Arc<anki::Knowledge>>,
    skip_hapax: bool,
    history_path: &Path,
    inputs: &HashMap<Language, Inputs>,
) -> Option<history::Entry> {
    let p = Profile::load(&profile::path_in(history_path, &e.id))?;
    let chars = p.chars.as_ref()?;
    let names: HashSet<String> = p.names.iter().cloned().collect();
    let known = knowledge.get(&p.language).map(|k| {
        history::Known::from_coverage(&BookCoverage::compute(
            k, chars, &p.words, &names, &p.parts, skip_hapax,
        ))
    });
    let mut e = e.clone();
    e.stats.known = known;
    e.inputs = inputs.get(&p.language)?.joined();
    Some(e)
}

/// The segmenter a Library row names (`SegmenterKind::label`); `Auto` if none does.
fn segmenter_by_label(label: &str) -> SegmenterKind {
    [
        SegmenterKind::Ckip,
        SegmenterKind::Ltp,
        SegmenterKind::Jieba,
        SegmenterKind::Kiwi,
        SegmenterKind::Sudachi,
    ]
    .into_iter()
    .find(|k| k.label() == label)
    .unwrap_or(SegmenterKind::Auto)
}

/// A digest of everything [`BookCoverage::compute`] and a Library row read
/// from the known words.
fn knowledge_digest(k: &anki::Knowledge) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let mut words: Vec<(&String, &Status)> = k.words.iter().collect();
    words.sort_unstable();
    h.update((words.len() as u64).to_le_bytes());
    for (w, s) in words {
        h.update((w.len() as u64).to_le_bytes());
        h.update(w.as_bytes());
        h.update(format!("{s:?}").as_bytes());
        h.update([0]);
    }
    let mut chars: Vec<(&char, &Status)> = k.chars.iter().collect();
    chars.sort_unstable();
    h.update((chars.len() as u64).to_le_bytes());
    for (c, s) in chars {
        h.update(u32::from(*c).to_le_bytes());
        h.update(format!("{s:?}").as_bytes());
        h.update([0]);
    }
    let mut stems: Vec<&String> = k.hada_stems.iter().collect();
    stems.sort_unstable();
    h.update((stems.len() as u64).to_le_bytes());
    for w in stems {
        h.update((w.len() as u64).to_le_bytes());
        h.update(w.as_bytes());
    }
    h.update([u8::from(k.fold), u8::from(k.from_anki)]);
    h.update(k.listed.source.as_bytes());
    h.finalize()[..12]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Size cover thumbnails are drawn at in the History table (points), and its row height.
const COVER_SIZE: (f32, f32) = (32.0, 44.0);
const COVER_ROW: f32 = 50.0;

/// A saved cover thumbnail as a texture; `None` if there is none (or it can't be read).
fn load_cover(ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
    let img = image::open(path).ok()?.into_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture(
        format!("cover {}", path.display()),
        pixels,
        egui::TextureOptions::LINEAR,
    ))
}
