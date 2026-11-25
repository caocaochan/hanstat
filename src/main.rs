#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use hanstat::analyze::{self, Prepared, Report};
use hanstat::anki::{self, BookCoverage, ByStatus, Difficulty, Status};
use hanstat::books;
use hanstat::cache::Cache;
use hanstat::discover::{self, Candidate};
use hanstat::extract::{self, Book, EncodingChoice, SectionKind};
use hanstat::history::{self, History};
use hanstat::known;
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
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

mod book_series;
mod chapter_lengths;
mod charts;
mod duchinese;
mod home;
mod library_books;
mod library_columns;
mod library_reading;
mod library_stats;
mod palette;
mod png_report;
mod raster;
mod read_books;
mod reader;
mod reading_stats;
mod secret;
mod shortcuts;
mod style_scale;
mod whats_new;

use library_columns::HistCol;
use library_stats::StatsState;
use shortcuts::{Action, Shortcuts};
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
    // What UiState keeps between runs, the window's size among it (its
    // place is not kept: it opens centred; see `App::place_window`).
    let window_file = hanstat::models::app_dir().join("window.ron");
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
        // Centred on the primary monitor, then sized on the first frame.
        centered: true,
        persist_window: false,
        persistence_path: Some(window_file),
        ..Default::default()
    };
    let result = eframe::run_native(
        "hanstat",
        options,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            let saved: Option<UiState> = cc.storage.and_then(|s| eframe::get_value(s, UI_KEY));
            let mut app = App::default();
            let ui = saved.unwrap_or_default();
            app.window_size = ui.window_size.map(egui::Vec2::from);
            app.window_maximized = ui.window_maximized;
            app.tab = tab.unwrap_or(ui.tab);
            app.history_tab = ui.history_tab;
            app.history_sort = ui.history_sort;
            if ui.library_hide_read && app.columns.cols.filter(HistCol::Read).is_none() {
                app.columns.cols.set_filter(
                    HistCol::Read,
                    library_columns::Filter {
                        values: false,
                        ..Default::default()
                    },
                );
                app.columns.save();
            }
            app.char_sort = ui.char_sort;
            app.word_sort = ui.word_sort;
            app.shortcuts = Shortcuts::from_saved(&app.settings.shortcuts);
            app.models = fetch_models(&cc.egui_ctx);
            app.updater.start(&cc.egui_ctx);
            app.whats_new_at_start();
            app.anki_init(&cc.egui_ctx);
            (app.history, app.history_error) = History::load();
            app.move_books(&cc.egui_ctx);
            (app.plan.settings, app.plan.error) = plan::Settings::load();
            if let Some(t) = history_tab {
                app.show_history = true;
                app.history_tab = t;
            }
            app.home = paths.is_empty();
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

/// Inter 4.1 (SIL OFL 1.1; its licence is beside it): Latin text on macOS
/// and Linux, which have no Segoe UI, in place of egui's Ubuntu Light, with
/// its semibold for headings and figures. TrueType outlines, which egui
/// draws correctly (not Apple's CFF ones; see `system_fonts`).
#[cfg(not(windows))]
static INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
#[cfg(not(windows))]
static INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");

/// egui ships no CJK glyphs; use the system's fonts as fallbacks, in order.
/// Characters none of them has still go to eframe's system font provider.
fn install_fonts(ctx: &egui::Context) {
    let (dir, candidates) = system_fonts();
    let mut fonts = egui::FontDefinitions::default();
    // Several faces can come from one `.ttc`: read each file once.
    let mut files: HashMap<&str, Option<egui::epaint::text::Blob>> = HashMap::new();
    for &(name, file, index) in candidates {
        let blob = files.entry(file).or_insert_with(|| {
            let mut bytes = std::fs::read(dir.join(file)).ok()?;
            // SimSun-ExtB's embedded bitmaps would be drawn white.
            reader::fonts::hide_bitmap_tables(&mut bytes);
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
    let proportional = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    // The semibold family (headings, figures): the faces below, then the
    // regular ones for whatever they lack.
    let mut bold = Vec::new();
    // Windows: Segoe UI for Latin, ahead of egui's Ubuntu, as Windows itself
    // pairs it with YaHei; their semibold and bold faces for the bold family.
    #[cfg(windows)]
    for (name, file, index, regular) in [
        ("segoe-ui", "segoeui.ttf", 0, true),
        ("segoe-ui-semibold", "seguisb.ttf", 0, false),
        ("yahei-bold", "msyhbd.ttc", 0, false),
    ] {
        let Ok(mut bytes) = std::fs::read(dir.join(file)) else {
            continue;
        };
        reader::fonts::hide_bitmap_tables(&mut bytes);
        let blob = Arc::new(bytes) as egui::epaint::text::Blob;
        fonts.font_data.insert(
            name.to_owned(),
            Arc::new(egui::FontData::from_blob(blob, index)),
        );
        if regular {
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, name.to_owned());
        } else {
            bold.push(name.to_owned());
        }
    }
    // macOS and Linux: the bundled Inter for Latin, likewise.
    #[cfg(not(windows))]
    for (name, bytes, regular) in [
        ("inter", INTER, true),
        ("inter-semibold", INTER_SEMIBOLD, false),
    ] {
        fonts.font_data.insert(
            name.to_owned(),
            Arc::new(egui::FontData::from_static(bytes)),
        );
        if regular {
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, name.to_owned());
        } else {
            bold.push(name.to_owned());
        }
    }
    bold.extend(proportional);
    fonts
        .families
        .insert(egui::FontFamily::Name(palette::BOLD.into()), bold);
    reader::fonts::set_base(fonts.clone());
    ctx.set_fonts(fonts);
    palette::install(ctx);
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
    /// The book's copy in the Library's folder, used from now on; the file
    /// opened if it could not be copied.
    path: PathBuf,
    /// Why it could not be copied.
    copy_error: Option<String>,
}

/// Library books added before it kept copies of its books, being copied
/// into its folder ([`App::move_books`]): each id with its copy.
struct BookMoves {
    rx: Receiver<Result<(String, PathBuf), String>>,
    done: Arc<AtomicUsize>,
    total: usize,
    failed: Vec<String>,
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

struct App {
    path: Option<PathBuf>,
    /// The segmenter choice.
    segmenter: SegmenterKind,
    encoding: EncodingChoice,
    /// Forced words, saved in `dictionary.txt`
    /// (see [`hanstat::user_dict_path`]).
    user_words: Vec<String>,
    /// The "Force a word" text box, the Forced words window, and a load/save error.
    force_input: String,
    forced_window: bool,
    /// The Suggested words tab lists every forced word, not just the newest few.
    forced_expanded: bool,
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
    /// another book is opened. Characters per minute.
    cpm: Option<f32>,
    /// `settings` changed by a drag still in progress: saved when it ends.
    settings_unsaved: bool,
    toast: Option<(String, f64)>,
    /// An image export being drawn: its message when done.
    image_job: Option<Receiver<String>>,

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
    /// Startup download of missing models.
    models: Option<Arc<ModelFetch>>,
    updater: Updater,
    /// Saved analysis results; `None` when turned off (`HANSTAT_NO_CACHE`).
    cache: Option<Cache>,
    /// Entries and bytes in `cache`, for the Notes tab (read when a book is analysed).
    cache_usage: (usize, u64),

    /// Books analysed before (history.json).
    history: History,
    history_error: Option<String>,
    /// Books being copied into the Library's folder.
    moves: Option<BookMoves>,
    /// The History view replaces the book's tabs.
    show_history: bool,
    /// Home ([`home`]) is shown in place of the open book's tabs (the
    /// Library still goes over it). Home is also shown with no book open.
    home: bool,
    home_view: home::HomeView,
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
    /// The words the books read teach together.
    across: read_books::Across,
    /// Library Stats › Reading.
    reading_view: library_reading::ReadingView,
    /// The Library's Books columns, filters and the user's own columns.
    columns: library_books::ColumnsState,
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
    /// The window's inner size once resized by hand, kept between runs;
    /// `None`: three quarters of the screen.
    window_size: Option<egui::Vec2>,
    /// Maximised when last closed (kept between runs).
    window_maximized: bool,
    /// Sized and centred at startup.
    window_placed: bool,
    /// Until then, size changes are the startup sizing landing, not the user.
    window_settle_until: f64,
    /// The window's inner size last frame, to notice a resize by hand.
    last_inner: Option<egui::Vec2>,
    /// [`App::lang_inputs`], with a quick hash of what they were made from
    /// and the known words they were made with.
    inputs_memo: Option<(u64, Option<Arc<anki::Knowledge>>, Inputs)>,
    /// Keyboard shortcuts ([`shortcuts`]), from `settings.shortcuts`.
    shortcuts: Shortcuts,
    /// The action whose new keys are being chosen in the Settings window,
    /// and what came of the last choice (keys refused, or taken from another action).
    shortcut_capture: Option<Action>,
    shortcut_note: Option<String>,
    /// Focus the filter (or the Library's search) when it is next drawn.
    focus_filter: bool,
    /// A text box had the keyboard last frame: Esc, which egui has already
    /// taken it from by now, left the box and closes nothing.
    typing_before: bool,
    /// The floating windows open, in the order they were opened.
    window_stack: Vec<Win>,
    /// The release notes since the version used last, on the first start
    /// of a newer one ([`whats_new`]).
    whats_new: Option<whats_new::WhatsNew>,
    /// The book open in the reader, which then fills the window.
    reader: Option<reader::Reader>,
    audiobook: Option<reader::Audiobook>,
    /// DuChinese stories: the window, imports and their updates.
    du: duchinese::DuState,
}

/// The floating windows, which the Close-window shortcut closes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Win {
    Anki,
    Known,
    Forced,
    Settings,
    Read,
    WhatsNew,
    DuChinese,
    DuSignIn,
}

impl Win {
    const ALL: [Win; 8] = [
        Win::Anki,
        Win::Known,
        Win::Forced,
        Win::Settings,
        Win::Read,
        Win::WhatsNew,
        Win::DuChinese,
        Win::DuSignIn,
    ];

    /// Its layer: the id given to its `egui::Window`.
    fn layer(self) -> egui::LayerId {
        let id = egui::Id::unique(match self {
            Win::Anki => "anki-window",
            Win::Known => "known-window",
            Win::Forced => "forced-window",
            Win::Settings => "settings-window",
            Win::Read => "read_dialog",
            Win::WhatsNew => whats_new::WINDOW_ID,
            Win::DuChinese => "duchinese-window",
            Win::DuSignIn => "duchinese-sign-in",
        });
        egui::LayerId::new(egui::Order::Middle, id)
    }
}

/// The book's tabs, in order.
const TABS: [(Tab, &str); 6] = [
    (Tab::Overview, "Overview"),
    (Tab::Characters, "Characters"),
    (Tab::Words, "Words"),
    (Tab::Sections, "Sections"),
    (Tab::Suggestions, "Suggested words"),
    (Tab::Notes, "Notes & method"),
];

impl Tab {
    /// The shortcut that shows it.
    fn action(self) -> Action {
        match self {
            Tab::Overview => Action::Overview,
            Tab::Characters => Action::Characters,
            Tab::Words => Action::Words,
            Tab::Sections => Action::Sections,
            Tab::Suggestions => Action::Suggestions,
            Tab::Notes => Action::Notes,
        }
    }
}

/// What a Library row in a language is computed from besides its book, its
/// sections and its segmenter ([`App::lang_inputs`]), as two digests:
/// `analysis` changes the counts (the build, the user dictionary, the word
/// lists given to a Chinese segmenter, character folding); `coverage` changes
/// only what is known of them (the build's coverage code, the known words,
/// the hapax setting). A row whose analysis digest is current is updated
/// from its saved word data alone.
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

/// The book the toolbar's 📕 Reader opens ([`App::last_read`]).
struct LastRead {
    id: String,
    path: PathBuf,
    title: String,
    traditional: bool,
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
    /// Read only, from builds before the Library's filters: becomes the
    /// ✔ Read column's "not read" filter.
    #[serde(skip_serializing)]
    library_hide_read: bool,
    char_sort: (CharCol, bool),
    word_sort: (WordCol, bool),
    /// See [`App::window_size`].
    window_size: Option<[f32; 2]>,
    window_maximized: bool,
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
            window_size: None,
            window_maximized: false,
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
    /// The inputs ([`App::lang_inputs`]) as last seen, and when they last changed.
    seen: Option<Inputs>,
    changed_at: f64,
    /// Stopped by the user with these inputs: not started again until they change.
    paused: Option<Inputs>,
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

/// The reading path ([`plan`]) of the History's books.
#[derive(Default)]
struct PlanState {
    settings: plan::Settings,
    error: Option<String>,
    /// History ids of the books with a profile, and what they were listed for.
    have: Option<((u64, usize), Vec<String>)>,
    /// History ids of the books with a profile but no word positions beside
    /// it, and what they were listed for.
    no_positions: Option<((u64, usize), HashSet<String>)>,
    /// The library last built, what from, and the files read for it.
    lib: Option<PlanLib>,
    /// What `result`, or the running job, was computed from.
    inputs: Option<PlanInputs>,
    result: Option<PlanDone>,
    job: Option<PlanJob>,
    /// When the last plan was started (`egui` time).
    started: f64,
}

/// A plan being made in the background.
struct PlanJob {
    rx: Receiver<PlanDone>,
    cancel: Arc<AtomicBool>,
    /// Its thread: the next plan waits for it, so only one runs at a time.
    thread: std::thread::JoinHandle<()>,
}

/// While books are being analysed, each one finished changes the word data:
/// the reading path is planned again at most this often (seconds), and not
/// while a plan is still running, instead of after every book.
const REPLAN_SECS: f64 = 10.0;

/// A file as read: when it was last changed and its length (`None`: missing).
type Stamp = Option<(std::time::SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// The library of `books` (History id, title) for `key`, with where their
/// words occur when that was saved with them. A book whose files are as
/// `old` read them is taken from it; the others are read. `None` if
/// cancelled.
fn plan_library(
    key: &LibKey,
    books: Vec<(String, String)>,
    history_path: &Path,
    old: Option<PlanLib>,
    cancel: &AtomicBool,
) -> Option<PlanLib> {
    let old = old.filter(|o| o.key.fold == key.fold && o.key.skip_hapax == key.skip_hapax);
    let index: HashMap<&str, usize> = old.as_ref().map_or_else(HashMap::new, |o| {
        o.lib
            .books
            .iter()
            .enumerate()
            .map(|(i, b)| (b.id.as_str(), i))
            .collect()
    });
    let mut files: HashMap<String, (Stamp, Stamp)> = HashMap::new();
    let mut src: Vec<(String, plan::Source)> = Vec::new();
    for (id, title) in books {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let p_path = profile::path_in(history_path, &id);
        let pos_path = profile::positions_path_in(history_path, &id);
        // Before reading: a file changed meanwhile is read again next time.
        let seen = (stamp(&p_path), stamp(&pos_path));
        let kept = old.as_ref().and_then(|o| {
            let &i = index.get(id.as_str())?;
            (o.files.get(&id) == Some(&seen)).then_some(i)
        });
        let s = match kept {
            Some(i) => plan::Source::Kept(i),
            None => {
                let Some(p) = Profile::load(&p_path) else {
                    continue;
                };
                let pos = profile::Positions::load(&pos_path, p.digest(), p.words.len());
                plan::Source::Read(p, pos)
            }
        };
        files.insert(id, seen);
        src.push((title, s));
    }
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    // Every book kept, in the same order: the same library.
    if let Some(o) = &old
        && src.len() == o.lib.books.len()
        && src.iter().enumerate().all(|(i, (t, s))| {
            matches!(s, plan::Source::Kept(k) if *k == i) && *t == o.lib.books[i].title
        })
    {
        return Some(PlanLib {
            key: key.clone(),
            lib: o.lib.clone(),
            files: Arc::new(files),
        });
    }
    let lib = plan::Library::rebuild(key.fold, key.skip_hapax, src, old.as_ref().map(|o| &*o.lib));
    Some(PlanLib {
        key: key.clone(),
        lib: Arc::new(lib),
        files: Arc::new(files),
    })
}

/// The reading path's library, with the profile and word-positions files
/// each book was read from (by History id): a book whose files are as they
/// were is taken from it when the library is built again.
#[derive(Clone)]
struct PlanLib {
    key: LibKey,
    lib: Arc<plan::Library>,
    files: Arc<HashMap<String, (Stamp, Stamp)>>,
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
    /// Learning while reading, the books the reader has a place in:
    /// (History id, file, spine index, offset, when).
    reading: Vec<(String, PathBuf, u32, u64, u64)>,
    /// Learning while reading, the word counts of the books read
    /// ([`read_books::Across::counts`], by `Arc` address; 0: none).
    read_counts: usize,
    /// The series the books name in their metadata, by History id
    /// ([`book_series`]).
    series: Vec<(String, hanstat::series::Meta)>,
}

impl PlanInputs {
    /// The same but for which books have word data, and that data: what
    /// changes with each book a batch finishes.
    fn same_but_word_data(&self, o: &PlanInputs) -> bool {
        let PlanInputs {
            lib,
            knowledge,
            params,
            books: _,
            manual,
            offsets,
            reading,
            read_counts,
            series,
        } = self;
        (lib.fold, lib.skip_hapax) == (o.lib.fold, o.lib.skip_hapax)
            && *knowledge == o.knowledge
            && *params == o.params
            && *manual == o.manual
            && *offsets == o.offsets
            && *reading == o.reading
            && *read_counts == o.read_counts
            && *series == o.series
    }
}

struct PlanDone {
    key: LibKey,
    lib: Arc<plan::Library>,
    /// What [`PlanLib::files`] says.
    files: Arc<HashMap<String, (Stamp, Stamp)>>,
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
    /// The inputs ([`App::lang_inputs`]) the books are analysed with.
    inputs: Option<Inputs>,
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
    /// A book analysed but not copied into the Library's folder: why.
    CopyFailed(String),
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

/// Text and colour of a book's script, as the Library's Language column
/// and the image exports show it.
fn script_badge(variant: Option<Variant>) -> (&'static str, Color32) {
    match variant {
        Some(Variant::Simplified) => (
            "简体中文",
            palette::pick(palette::MOCHA.red, palette::ink::RED),
        ),
        Some(Variant::Traditional) => (
            "繁體中文",
            palette::pick(palette::MOCHA.peach, palette::ink::PEACH),
        ),
        Some(Variant::Mixed) => (
            "简繁混合",
            palette::pick(palette::MOCHA.maroon, palette::ink::MAROON),
        ),
        _ => (
            "中文",
            palette::pick(palette::MOCHA.flamingo, palette::ink::FLAMINGO),
        ),
    }
}

fn status_colour(s: Status) -> Color32 {
    use palette::{MOCHA as M, ink};
    match s {
        Status::Mature => palette::pick(M.green, ink::GREEN),
        Status::Young => palette::pick(M.teal, ink::TEAL),
        Status::Listed => palette::pick(M.lavender, ink::LAVENDER),
        Status::Learning => palette::pick(M.yellow, ink::YELLOW),
        Status::New => palette::pick(M.blue, ink::BLUE),
        Status::NotInAnki => palette::weak(),
    }
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
            segmenter: SegmenterKind::Auto,
            encoding: EncodingChoice::Auto,
            user_words: Vec::new(),
            force_input: String::new(),
            forced_window: false,
            forced_expanded: false,
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
            settings_unsaved: false,
            toast: None,
            image_job: None,
            suggestions: None,
            suggest_rx: None,
            discover_due: None,
            discover_cancel: None,
            selected: HashSet::new(),
            anki: AnkiState::default(),
            anki_filter: AnkiFilter::All,
            known: KnownState::default(),
            models: None,
            updater: Updater::default(),
            cache: Cache::open(),
            cache_usage: (0, 0),
            history: History::default(),
            history_error: None,
            moves: None,
            show_history: false,
            home: true,
            home_view: Default::default(),
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
            across: Default::default(),
            reading_view: Default::default(),
            columns: library_books::ColumnsState::default(),
            exists: Exists::default(),
            removed: None,
            settings_window: false,
            char_sort: (CharCol::Rank, false),
            word_sort: (WordCol::Rank, false),
            char_rows: None,
            word_rows: None,
            report_gen: 0,
            window_size: None,
            window_maximized: false,
            window_placed: false,
            window_settle_until: 0.0,
            last_inner: None,
            inputs_memo: None,
            shortcuts: Shortcuts::default(),
            shortcut_capture: None,
            shortcut_note: None,
            focus_filter: false,
            typing_before: false,
            window_stack: Vec::new(),
            whats_new: None,
            reader: None,
            audiobook: None,
            du: duchinese::DuState::default(),
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
        let words = self.user_words.clone();
        // Imported word lists join words the segmenter split.
        let known_words = self.known.settings.segmenter_words();
        // Reuse the already-extracted book if only segmentation options changed.
        let cached = self
            .book
            .clone()
            .filter(|_| self.path.as_deref() == Some(path.as_path()));
        let books_dir = self.history.books();
        std::thread::spawn(move || {
            let res = (|| {
                let book = match cached {
                    Some(b) => b,
                    None => Arc::new(extract::load(&path, enc)?),
                };
                // First use of a model: download it (pinned + checksummed) with visible progress.
                let (tr, si) = analyze::detect_variant(&book);
                let resolved = kind.resolve(hanstat::script::classify(tr, si));
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
                // The Library's own copy, read from now on.
                let (path, file_id, copy_error) = match books::import(&books_dir, &path, None) {
                    Ok(i) => {
                        if i.copied && c3.load(Ordering::Relaxed) {
                            // Replaced by another analysis meanwhile: not kept.
                            books::delete(&books_dir, &i.id);
                            return Err(analyze::CANCELLED.to_owned());
                        }
                        (i.path, Some(i.id), None)
                    }
                    Err(e) => (path.clone(), history::file_id(&path).ok(), Some(e)),
                };
                // The cover's thumbnail for the History view; the same file always has the same cover.
                if let (Some(id), Some(cover)) = (&file_id, &book.cover) {
                    let dest = history::cover_path(id);
                    if !dest.is_file() {
                        let _ = history::save_cover(cover, &dest);
                    }
                    // And a large one for Home.
                    let large = history::large_cover_path(id);
                    if !large.is_file() {
                        let _ = history::save_large_cover(cover, &large);
                    }
                }
                Ok(Loaded {
                    book,
                    prep: Arc::new(prep),
                    file_id,
                    path,
                    copy_error,
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
        self.home = false;
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

    /// Load the saved settings: forced words, Anki, known words.
    fn anki_init(&mut self, ctx: &egui::Context) {
        let (w, warn) = hanstat::load_user_dict();
        self.user_words = w;
        self.dict_error = warn;
        let (s, warn) = anki::Settings::load();
        self.anki.settings = s;
        self.anki.error = warn;
        let (k, warn) = known::Settings::load();
        self.known.settings = k;
        self.known.error = warn;
        self.refresh_anki_coverage();
        self.anki.profiles = anki::find_profiles();
        if self.anki.settings.enabled && self.anki.settings.collection.is_some() {
            // Clears copies left behind by earlier runs before making its own.
            self.anki_open(ctx);
        } else {
            std::thread::spawn(|| anki::remove_stale_copies(&anki::copies_root(), None));
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
        anki::SegConfig::new(
            self.segmenter,
            self.prep.as_ref().map(|p| p.segmenter_kind),
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
        self.anki.busy = "Classifying Anki notes…";
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
            .is_some_and(|r| r.chars_folded != self.fold_chars())
        {
            self.rebuild_report();
        } else {
            self.refresh_anki_coverage();
        }
    }

    /// Count the Traditional and Simplified forms of a Chinese character as
    /// one: the "Match Traditional ↔ Simplified" option.
    fn fold_chars(&self) -> bool {
        self.anki.settings.match_across_scripts
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
        r.anki = self
            .known
            .merged
            .as_ref()
            .map(|k| BookCoverage::compute(k, &r.char_freq, &r.word_freq, &names, skip_hapax));
        if r.anki.is_none() {
            self.anki_filter = AnkiFilter::All;
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
        // Where its words occur, with the profile they were made with.
        let pos_path = self.history.positions(id);
        let positions = match (&self.book, &self.prep) {
            (Some(b), Some(prep)) if profile.is_some() || !pos_path.is_file() => Some((
                profile::Positions::from_analysis(b, prep, &self.included, &r.word_freq),
                profile.as_ref().map_or_else(
                    || Profile::from_report(id, r, names).digest(),
                    |p| p.digest(),
                ),
            )),
            _ => None,
        };
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
        entry.inputs = self.lang_inputs().map(|i| i.joined()).unwrap_or_default();
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
        if let Some((pos, digest)) = positions {
            match pos.save(&pos_path, digest) {
                // The reading path reads them.
                Ok(()) => self.profiles_version += 1,
                Err(e) => self.history_error = Some(e),
            }
        }
    }

    fn anki_window(&mut self, ctx: &egui::Context) {
        let mut open = self.anki.window;
        let mut changed = false;
        let mut reopen = false;
        egui::Window::new("Anki")
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
                    changed |= ui
                        .checkbox(&mut a.settings.match_across_scripts, "Match Traditional ↔ Simplified")
                        .on_hover_text("Fold both your cards and the book to Simplified (character level) before matching.")
                        .changed();
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
                                " ({} have no Chinese in the ticked fields)",
                                fmt_int(s.notes_without_chinese)
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
                    status_grid(ui, "anki-summary", &[("Cards", &s.cards), ("Words", &s.words), ("Characters", &s.chars)]);
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "{0} Learning and relearning cards are not counted as known. \
                         {2} HTML, sound tags and [readings] are ignored. \
                         The collection is read from a temporary copy and never modified. Choices are saved to {1}.",
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
                        anki::Settings::path().display(),
                        "A ticked field holding a word counts as that word; one holding a sentence is split into words with the book's segmenter.",
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
        self.dict_error = hanstat::save_user_dict(&self.user_words).err();
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
    /// Suggested words tab and the Forced words window. With `collapse`, only
    /// the newest `FORCED_SHOWN` are listed until the "… N more" button is
    /// clicked.
    fn forced_words_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, collapse: bool) {
        const FORCED_SHOWN: usize = 5;
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
                let n = self.user_words.len();
                ui.label(format!("{n} forced:"));
                // Newest are appended last, so a collapsed list shows the tail.
                let hidden = if collapse && !self.forced_expanded {
                    n.saturating_sub(FORCED_SHOWN)
                } else {
                    0
                };
                for w in &self.user_words[hidden..] {
                    let b = egui::Button::new(format!("{w} ✖")).small();
                    if ui
                        .add_enabled(!busy, b)
                        .on_hover_text("Stop forcing this word")
                        .clicked()
                    {
                        words.retain(|x| x != w);
                    }
                }
                if hidden > 0 {
                    if ui
                        .add(egui::Button::new(format!("… {hidden} more")).small())
                        .on_hover_text("Show every forced word (the newest are shown)")
                        .clicked()
                    {
                        self.forced_expanded = true;
                    }
                } else if collapse
                    && n > FORCED_SHOWN
                    && ui.add(egui::Button::new("Show fewer").small()).clicked()
                {
                    self.forced_expanded = false;
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
                self.forced_words_ui(ui, ctx, false);
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
                if ui
                    .button("Changelog")
                    .on_hover_text("What changed in every release, newest first.")
                    .clicked()
                {
                    self.whats_new = Some(whats_new::WhatsNew::all());
                    ui.close();
                }
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
            if st.store.get(&e.id).is_some() || !st.asked.insert(e.id.clone()) {
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
            let measured = reading_stats::measured_now();
            egui::Grid::new("speeds").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                let s = &mut self.settings;
                let mut changed = false;
                ui.label("Chinese");
                ui.horizontal(|ui| {
                    // Measured in the reader unless overridden; the value set
                    // here also stands in until a speed is measured.
                    let by_hand = s.cpm_by_hand || measured.is_none();
                    if by_hand {
                        changed |= ui
                            .add(egui::DragValue::new(&mut s.cpm).range(50.0..=2000.0).speed(5.0).suffix(" chars/min"))
                            .on_hover_text("Chinese characters per minute")
                            .changed();
                    } else {
                        let mut v = measured.unwrap_or_default().round() as f32;
                        ui.add_enabled(false, egui::DragValue::new(&mut v).suffix(" chars/min"))
                            .on_disabled_hover_text("Measured in the reader: characters read ÷ minutes read over the last 30 days");
                    }
                    changed |= ui
                        .checkbox(&mut s.cpm_by_hand, "Override")
                        .on_hover_text(match measured {
                            Some(v) => format!("Use a speed set here instead of the {v:.0} chars/min measured in the reader"),
                            None => "Use the speed set here even once your speed is measured in the reader".to_owned(),
                        })
                        .changed();
                });
                ui.end_row();
                ui.label("");
                let note = match (s.cpm_by_hand, measured) {
                    (false, Some(_)) => "Measured in the reader over the last 30 days".to_owned(),
                    (false, None) => "Until you've read 10 minutes in the reader; then measured there".to_owned(),
                    (true, Some(v)) => format!("Set by hand (measured: {v:.0} chars/min)"),
                    (true, None) => "Set by hand".to_owned(),
                };
                ui.label(RichText::new(note).weak().small());
                ui.end_row();
                if changed {
                    self.settings_unsaved = true;
                }
            });
            ui.label(
                RichText::new(
                    "For the reading times in the Library and on each book's Overview, where a book's speed can be changed on its own to try another, and for the time left in the reader.",
                )
                .weak()
                .small(),
            );
            ui.separator();
            self.shortcuts_ui(ui);
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
            let books = &self.history.entries;
            let rated = books.iter().filter(|e| self.style.store.get(&e.id).is_some()).count();
            ui.label(format!(
                "{rated} of the {} books in the Library are rated.",
                books.len()
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
            let skipped: Vec<String> = books
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
            let unrated = books.len() - rated;
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
        self.columns.finish_delete();
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
                        self.cpm = None;
                    }
                    self.cache_usage = self.cache.as_ref().map_or((0, 0), Cache::usage);
                    self.book = Some(l.book);
                    self.prep = Some(l.prep);
                    // The old report is not this book's.
                    self.report = None;
                    self.report_id = None;
                    self.book_id = l.file_id;
                    self.path = Some(l.path);
                    match l.copy_error {
                        Some(e) => self.set_error("copy", e),
                        None => self.clear_error("copy"),
                    }
                    self.history_touch = true;
                    self.rebuild_report();
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
                library_hide_read: false,
                char_sort: self.char_sort,
                word_sort: self.word_sort,
                window_size: self.window_size.map(<[f32; 2]>::from),
                window_maximized: self.window_maximized,
            },
        );
    }

    fn on_exit(&mut self) {
        if let Some(r) = &mut self.reader {
            r.end_stats();
            r.save();
        }
        self.finish_removal();
        self.columns.finish_delete();
        hanstat::saver::flush();
        reading_stats::flush();
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
        self.poll_across(&ctx);
        self.poll_stats(&ctx);
        self.poll_image(&ctx);
        // Counted only while shown: it reads every book's word data.
        if self.show_history && self.history_tab == HistTab::Stats {
            self.refresh_stats(&ctx);
        }
        self.poll_batch(&ctx);
        self.poll_moves(&ctx);
        // Each EPUB's series metadata, read once ([`book_series`]).
        book_series::want(
            self.history
                .entries
                .iter()
                .filter(|e| e.stats.format == "EPUB")
                .map(|e| (e.id.as_str(), e.path.as_path())),
            &ctx,
        );
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
        self.columns.tick(now, &ctx);
        if let Some(e) = self.columns.error.take() {
            self.set_error("columns", e);
        }
        if let Some(e) = hanstat::saver::take_error() {
            self.set_error("save", e);
        }
        let set_aside = hanstat::saver::take_notices();
        if !set_aside.is_empty() {
            self.set_error("unreadable", set_aside.join("\n"));
        }
        self.place_window(&ctx, now);
        if self.settings_unsaved && !ctx.input(|i| i.pointer.any_down()) {
            self.settings_unsaved = false;
            if let Err(e) = self.settings.save() {
                self.set_error("settings", e);
            }
        }
        self.style_tick(&ctx);
        self.du_tick(&ctx);

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if self.reader.is_none() && self.audiobook.is_none() && !dropped.is_empty() {
            self.open_files(dropped, &ctx);
        }
        // The reader fills the window while a book is open in it.
        let (cpm, by_hand) = (self.cpm(), self.settings.cpm_by_hand);
        let notice = self
            .reader
            .as_ref()
            .and_then(|r| self.du_reader_notice(r.book_id(), r.on_last_chapter()));
        if let Some(r) = &mut self.reader {
            r.cpm = cpm;
            r.cpm_by_hand = by_hand;
            r.notice = notice;
            let open = r.ui(ui);
            if r.take_notice_click() {
                self.du_reload(&ctx);
                ctx.request_repaint();
                return;
            }
            let Some(r) = &mut self.reader else { return };
            let finished = r.take_finished().then(|| r.book_id().to_owned());
            // A Library book read to the end: offer Mark read, unless it is.
            if let Some(id) = finished
                && self.history.entries.iter().any(|e| e.id == id)
                && !self.read_dates().contains_key(&id)
            {
                self.open_read_dialog(&id);
            }
            if self.read_dialog.is_some() {
                self.read_window(&ctx);
            }
            if !open {
                if let Some(r) = self.reader.take() {
                    r.close(&ctx);
                }
                ctx.request_repaint();
            }
            return;
        }
        if self.audiobook.is_none() {
            self.handle_shortcuts(&ctx);
        }

        let on_home = self.on_home();
        egui::Panel::top("toolbar").frame(palette::bar(true)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                palette::compact(ui);
                if ui
                    .button("📂 Open…")
                    .on_hover_text(self.shortcuts.hint(&ctx, Action::Open, "Open a book, or select several: each is analysed and added to the Library, and the last one is shown"))
                    .clicked()
                {
                    self.open_dialog(&ctx);
                }
                if ui
                    .selectable_label(on_home, "🏠 Home")
                    .on_hover_text(self.shortcuts.hint(&ctx, Action::Home, "The book you're reading, today's reading and your totals"))
                    .clicked()
                {
                    self.go_home();
                }
                // Home has no book: what applies to one isn't offered there.
                if !on_home {
                    ui.separator();
                    ui.label("Segmenter:");
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

                    let hover = "Words always segmented as single words, saved between runs. Add them here or from the Suggested words tab.";
                    if ui
                        .selectable_label(self.forced_window, format!("📖 {} forced words", self.user_words.len()))
                        .on_hover_text(self.shortcuts.hint(&ctx, Action::ForcedWords, hover))
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
                    .on_hover_text(self.shortcuts.hint(&ctx, Action::Anki, "Compare the book with your Anki cards (young and mature)"))
                    .clicked()
                {
                    self.toggle_anki(&ctx);
                }

                let is_txt =
                    !on_home && self.book.as_ref().is_some_and(|b| b.format == "TXT");
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

                if self.report.is_some() || !self.history.entries.is_empty() {
                    ui.separator();
                    let mut image = None;
                    ui.menu_button("💾 Export", |ui| {
                        let busy = self.image_busy();
                        let library_button = |ui: &mut egui::Ui| {
                            ui.add_enabled(!busy, egui::Button::new("Library as image (PNG)…"))
                                .on_hover_text("One long picture of the Library: its totals, its vocabulary and every book")
                                .on_disabled_hover_text("An image is being saved")
                                .clicked()
                        };
                        let Some(r) = self.report.as_ref().filter(|_| !on_home) else {
                            if library_button(ui) {
                                image = Some(png_report::ImageExport::Library);
                                ui.close();
                            }
                            return;
                        };
                        let stem = self.path.as_deref().map(file_stem).unwrap_or_else(|| "hanstat".into());
                        let mut msg = None;
                        if ui.button("Character frequency (CSV)…").clicked() {
                            msg = save(&format!("{stem}-characters.csv"), "csv", analyze::freq_csv(&r.char_freq, "character"));
                        }
                        if ui.button("Word frequency (CSV)…").clicked() {
                            msg = save(&format!("{stem}-words.csv"), "csv", analyze::freq_csv(&r.word_freq, "word"));
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
                                .map(|((w, _), _)| w.as_str())
                                .collect();
                            msg = save(&format!("{stem}-unknown-words.txt"), "txt", list.join("\n") + "\n");
                        }
                        if !self.user_words.is_empty() && ui.button("Forced words as user dictionary (TXT)…").clicked() {
                            msg = save(&format!("{stem}-dictionary.txt"), "txt", self.user_words.join("\n") + "\n");
                        }
                        ui.separator();
                        if ui
                            .add_enabled(!busy, egui::Button::new("Book as image (PNG)…"))
                            .on_hover_text("A picture of this book's Overview: its cover, figures, difficulty and most frequent unknown words")
                            .on_disabled_hover_text("An image is being saved")
                            .clicked()
                        {
                            image = Some(png_report::ImageExport::Book);
                            ui.close();
                        }
                        if !self.history.entries.is_empty() && library_button(ui) {
                            image = Some(png_report::ImageExport::Library);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Copy unique characters").clicked() {
                            ctx.copy_text(r.char_freq.iter().map(|x| x.0).collect());
                            msg = Some(format!("Copied {} characters", r.char_freq.len()));
                        }
                        if ui.button("Copy unique words (one per line)").clicked() {
                            ctx.copy_text(r.word_freq.iter().map(|x| x.0.as_str()).collect::<Vec<_>>().join("\n"));
                            msg = Some(format!("Copied {} words", r.word_freq.len()));
                        }
                        if let Some(m) = msg {
                            self.toast = Some((m, ui.input(|i| i.time) + 3.0));
                            ui.close();
                        }
                    });
                    if let Some(what) = image {
                        self.export_image(&ctx, what);
                    }
                }

                ui.separator();
                let label = if self.known.settings.is_empty() {
                    "📚 HSK, TOCFL & word lists…".to_owned()
                } else {
                    format!("📚 Known: {}", self.known.settings.label())
                };
                let hover = "Mark HSK 3.0, HSK 2.0 or TOCFL levels and imported word lists as known, in addition to your Anki cards";
                if ui
                    .selectable_label(self.known.window, label)
                    .on_hover_text(self.shortcuts.hint(&ctx, Action::KnownWords, hover))
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
                    .on_hover_text(self.shortcuts.hint(&ctx, Action::Library, "Every book analysed before, with its statistics, and a reading path through them"))
                    .clicked()
                {
                    self.show_history = !self.show_history;
                }
                let last = self.last_read();
                let reader_button = ui.add_enabled(last.is_some(), egui::Button::new("📕 Reader"));
                if let Some(b) = last {
                    if reader_button
                        .on_hover_text(format!("Go back to {} where you left it", b.title))
                        .clicked()
                    {
                        match reader::Reader::open(&b.path, Some(&b.id), b.traditional, &ctx) {
                            Ok(r) => self.reader = Some(r),
                            Err(e) => self.set_error("reader", e),
                        }
                    }
                } else {
                    reader_button.on_disabled_hover_text("No Library book read in the reader yet");
                }

            });
        });

        egui::Panel::bottom("status").frame(palette::bar(false)).show(ui, |ui| {
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
                    .on_hover_text(self.shortcuts.hint(&ctx, Action::Settings, "Settings, keyboard shortcuts"))
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
                self.columns.undo_ui(ui);
                // The space left of them, in that row (not the panel's height).
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true), |ui| {
                if self.batch.is_some() {
                    self.batch_status(ui);
                    ui.separator();
                }
                if let Some(m) = &self.moves {
                    ui.spinner();
                    ui.label(format!(
                        "Copying books into the Library: {} of {}",
                        (m.done.load(Ordering::Relaxed) + 1).min(m.total),
                        m.total
                    ))
                    .on_hover_text(
                        "The Library now keeps its own copy of each book, so the files you added them from can be moved or deleted. \
                         Books added before are copied once; the originals are not touched",
                    );
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

        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(ui.style())
                    .inner_margin(egui::Margin::symmetric(18, 12)),
            )
            .show(ui, |ui| {
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
                if self.on_home() {
                    self.home_page(ui, &ctx);
                    return;
                }
                if self.report.is_none() {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("Analysing…").size(22.0).weak());
                    });
                    return;
                }
                let tab_before = self.tab;
                let shown: Vec<_> = TABS.into_iter().collect();
                let shortcuts = &self.shortcuts;
                palette::tabs(ui, &mut self.tab, shown, 15.0, |t, r| {
                    if let Some(k) = shortcuts.text(&ctx, t.action()) {
                        r.on_hover_text(k);
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
                ui.add_space(6.0);
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
        self.du_windows(&ctx);
        if let Some(w) = &self.whats_new
            && !whats_new::window(&ctx, w)
        {
            self.whats_new = None;
        }
        if let Some(mut dialog) = self.audiobook.take() {
            match dialog.show(&ctx) {
                reader::AudiobookAction::Keep => self.audiobook = Some(dialog),
                reader::AudiobookAction::Close => {}
                reader::AudiobookAction::Read(reading) => match reading.open(&ctx) {
                    Ok(reader) => self.reader = Some(reader),
                    Err(error) => self.set_error("reader", error),
                },
            }
        }
        // Asked for but not drawn (no filter shown): not kept for later.
        self.focus_filter = false;
        self.typing_before = ctx.text_edit_focused();
    }
}

impl App {
    /// Whether Home is in the central panel: chosen, or no book to show
    /// (none open and none on its way).
    fn on_home(&self) -> bool {
        let opening = self.job.is_some()
            || self
                .batch
                .as_ref()
                .is_some_and(|b| b.mode == BatchMode::Open);
        !self.show_history && (self.home || self.report.is_none() && !opening)
    }

    fn go_home(&mut self) {
        self.home = true;
        self.show_history = false;
    }

    /// At startup, centre the window on the screen at the size chosen by hand
    /// (or three quarters of the screen), or maximise it as it was; then
    /// remember a size or maximised state the user picks. Its place is not
    /// kept: it always opens centred.
    fn place_window(&mut self, ctx: &egui::Context, now: f64) {
        let (inner, outer, monitor, maxed, full) = ctx.input(|i| {
            let v = i.viewport();
            (
                v.inner_rect,
                v.outer_rect,
                v.monitor_size,
                v.maximized == Some(true),
                v.fullscreen == Some(true),
            )
        });
        let (Some(inner), Some(outer), Some(monitor)) = (inner, outer, monitor) else {
            return;
        };
        if !self.window_placed {
            self.window_placed = true;
            self.window_settle_until = now + 1.5;
            let chrome = (outer.size() - inner.size()).max(egui::Vec2::ZERO);
            let fits = (monitor - chrome).max(egui::vec2(200.0, 200.0));
            let size = self
                .window_size
                .unwrap_or(monitor * 0.75 - chrome)
                .min(fits);
            // Centred on the primary monitor, where eframe opened it (origin 0, 0).
            let pos = ((monitor - size - chrome) * 0.5).max(egui::Vec2::ZERO);
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos.to_pos2()));
            // Restoring it later gives that size, centred.
            if self.window_maximized {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            }
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(1.6));
            return;
        }
        let size = inner.size();
        if now >= self.window_settle_until {
            self.window_maximized = maxed;
            if !maxed
                && !full
                && let Some(was) = self.last_inner
                && (was - size).length() > 1.0
            {
                self.window_size = Some(size);
            }
        }
        self.last_inner = Some(size);
    }

    /// At startup: open the What's new window if this version is newer than
    /// the one started last, and record this one.
    fn whats_new_at_start(&mut self) {
        let current = update::current_version();
        let recorded = self.updater.settings.last_version.clone();
        let (sections, record) =
            whats_new::at_start(whats_new::CHANGELOG, recorded.as_deref(), current);
        if record {
            self.updater.settings.last_version = Some(update::format_version(current));
            if let Err(e) = self.updater.settings.save() {
                self.set_error("update", e);
            }
        }
        if let Some(since) = recorded.as_deref().and_then(update::parse_version)
            && !sections.is_empty()
        {
            self.whats_new = Some(whats_new::WhatsNew {
                since: Some(since),
                sections,
            });
        }
    }

    /// Keyboard shortcuts pressed this frame, before anything is drawn; or,
    /// while new keys are being chosen in the Settings window, those keys.
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let open: Vec<Win> = Win::ALL.into_iter().filter(|&w| self.win_open(w)).collect();
        self.window_stack.retain(|w| open.contains(w));
        for w in open {
            if !self.window_stack.contains(&w) {
                self.window_stack.push(w);
            }
        }
        if let Some(a) = self.shortcut_capture {
            if !self.settings_window {
                self.shortcut_capture = None;
            } else {
                match shortcuts::capture(ctx) {
                    shortcuts::Captured::Waiting => return,
                    shortcuts::Captured::Cancel => self.shortcut_note = None,
                    shortcuts::Captured::Refused(why) => {
                        self.shortcut_note = Some(format!("{why} Choose other keys."));
                        return;
                    }
                    shortcuts::Captured::Keys(k) => {
                        self.shortcut_note = self.shortcuts.set(a, Some(k)).map(|b| {
                            format!(
                                "{} was taken from “{}”, which now has none.",
                                shortcuts::display(ctx, k),
                                b.label()
                            )
                        });
                        self.save_shortcuts();
                    }
                }
                self.shortcut_capture = None;
                return;
            }
        }
        let typing = self.typing_before || ctx.text_edit_focused();
        let plain_blocked = typing || egui::Popup::is_any_open(ctx);
        for a in self.shortcuts.pressed(ctx, plain_blocked) {
            self.run_action(a, ctx);
        }
    }

    fn save_shortcuts(&mut self) {
        self.settings.shortcuts = self.shortcuts.to_saved();
        if let Err(e) = self.settings.save() {
            self.set_error("settings", e);
        }
    }

    fn run_action(&mut self, a: Action, ctx: &egui::Context) {
        match a {
            Action::Open => self.open_dialog(ctx),
            Action::Home => self.go_home(),
            Action::Library => self.show_history = !self.show_history,
            Action::Filter => {
                if self.show_history || self.report.is_none() {
                    self.show_history = true;
                    if self.history_tab == HistTab::Stats {
                        self.history_tab = HistTab::Books;
                    }
                } else if !matches!(self.tab, Tab::Characters | Tab::Words) {
                    self.switch_tab(Tab::Words, ctx);
                }
                self.focus_filter = true;
            }
            Action::NextTab | Action::PrevTab => {
                let next = a == Action::NextTab;
                if self.show_history {
                    const H: [HistTab; 3] = [HistTab::Books, HistTab::Path, HistTab::Stats];
                    let i = H.iter().position(|&t| t == self.history_tab).unwrap_or(0);
                    self.history_tab = H[(i + if next { 1 } else { H.len() - 1 }) % H.len()];
                } else if self.report.is_some() && !self.on_home() {
                    let step = if next { 1 } else { TABS.len() - 1 };
                    let i = TABS.iter().position(|&(t, _)| t == self.tab).unwrap_or(0);
                    self.switch_tab(TABS[(i + step) % TABS.len()].0, ctx);
                }
            }
            Action::KnownWords => self.known.window = !self.known.window,
            Action::Anki => self.toggle_anki(ctx),
            Action::ForcedWords => self.forced_window = !self.forced_window,
            Action::Settings => self.settings_window = !self.settings_window,
            Action::CloseWindow => self.close_front_window(ctx),
            // Handled by the Words table.
            Action::MarkKnown => {}
            Action::Overview
            | Action::Characters
            | Action::Words
            | Action::Sections
            | Action::Suggestions
            | Action::Notes => {
                if let Some(&(t, _)) = TABS.iter().find(|(t, _)| t.action() == a)
                    && self.report.is_some()
                {
                    self.show_history = false;
                    self.home = false;
                    self.switch_tab(t, ctx);
                }
            }
        }
    }

    /// Show a book tab. Marking words or importing lists changes how the
    /// book is segmented; it's redone on returning to the Overview, not per click.
    fn switch_tab(&mut self, t: Tab, ctx: &egui::Context) {
        let before = std::mem::replace(&mut self.tab, t);
        if t == Tab::Overview && before != Tab::Overview && self.known_resegment_due() {
            self.rerun(ctx, false);
        }
    }

    fn win_open(&self, w: Win) -> bool {
        match w {
            Win::Anki => self.anki.window,
            Win::Known => self.known.window,
            Win::Forced => self.forced_window,
            Win::Settings => self.settings_window,
            Win::Read => self.read_dialog.is_some(),
            Win::WhatsNew => self.whats_new.is_some(),
            Win::DuChinese => self.du.open,
            Win::DuSignIn => self.du.signing_in(),
        }
    }

    /// Close the floating window in front: the one egui has on top, or else
    /// the one opened last.
    fn close_front_window(&mut self, ctx: &egui::Context) {
        let top = ctx.top_layer_id();
        let w = Win::ALL
            .into_iter()
            .find(|&w| self.win_open(w) && Some(w.layer()) == top)
            .or_else(|| self.window_stack.last().copied());
        match w {
            Some(Win::Anki) => self.anki.window = false,
            Some(Win::Known) => self.known.window = false,
            Some(Win::Forced) => self.forced_window = false,
            Some(Win::Settings) => self.settings_window = false,
            Some(Win::Read) => self.read_dialog = None,
            Some(Win::WhatsNew) => self.whats_new = None,
            Some(Win::DuChinese) => self.du.open = false,
            Some(Win::DuSignIn) => self.du.close_sign_in(),
            None => {}
        }
    }

    fn toggle_anki(&mut self, ctx: &egui::Context) {
        self.anki.window = !self.anki.window;
        if self.anki.window && self.anki.col.is_none() && self.anki.rx.is_none() {
            self.anki_open_default(ctx);
        }
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        if let Some(files) = rfd::FileDialog::new()
            .add_filter("E-books and text", &["epub", "txt", "text"])
            .add_filter("All files", &["*"])
            .pick_files()
        {
            self.open_files(files, ctx);
        }
    }

    /// Focus a filter box just drawn, if asked to ([`App::focus_filter`]),
    /// with its text selected so that typing replaces it.
    fn focus_filter_box(asked: &mut bool, ui: &egui::Ui, r: &egui::Response, text: &str) {
        if std::mem::take(asked) {
            r.request_focus();
            let mut state = egui::TextEdit::load_state(ui.ctx(), r.id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text_selection::CCursorRange::select_all_str(
                    text,
                )));
            egui::TextEdit::store_state(ui.ctx(), r.id, state);
        }
    }

    /// The Keyboard shortcuts section of the Settings window.
    fn shortcuts_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let shown = egui::CollapsingHeader::new("Keyboard shortcuts")
            .id_salt("shortcuts")
            .show(ui, |ui| {
                ui.label(
                    RichText::new(
                        "Click a shortcut, then press the new keys (Esc keeps the old ones). \
                         Keys without Ctrl or Alt do nothing while you type in a text box.",
                    )
                    .weak()
                    .small(),
                );
                // Taller than the rest of the window: it scrolls.
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                egui::Grid::new("shortcut-grid")
                    .num_columns(3)
                    .spacing([12.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for a in Action::ALL {
                            let r = ui.label(a.label());
                            if let Some(h) = a.hover() {
                                r.on_hover_text(h);
                            }
                            let capturing = self.shortcut_capture == Some(a);
                            let text = if capturing {
                                RichText::new("Press keys…").italics()
                            } else {
                                match self.shortcuts.text(&ctx, a) {
                                    Some(k) => RichText::new(k),
                                    None => RichText::new("none").weak(),
                                }
                            };
                            if ui
                                .add(
                                    egui::Button::new(text)
                                        .selected(capturing)
                                        .min_size(egui::vec2(110.0, 0.0)),
                                )
                                .on_hover_text(if capturing {
                                    "Press the new keys; Esc keeps the old ones"
                                } else {
                                    "Change"
                                })
                                .clicked()
                            {
                                self.shortcut_capture = if capturing { None } else { Some(a) };
                                self.shortcut_note = None;
                            }
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(
                                        self.shortcuts.get(a).is_some(),
                                        egui::Button::new("✖").small(),
                                    )
                                    .on_hover_text("No keys")
                                    .clicked()
                                {
                                    self.shortcuts.set(a, None);
                                    self.shortcut_capture = None;
                                    self.shortcut_note = None;
                                    self.save_shortcuts();
                                }
                                if ui
                                    .add_enabled(
                                        !self.shortcuts.is_default(a),
                                        egui::Button::new("⟲").small(),
                                    )
                                    .on_hover_text("Back to the default")
                                    .clicked()
                                {
                                    let d = Shortcuts::default().get(a);
                                    self.shortcut_note = self.shortcuts.set(a, d).map(|b| {
                                        format!(
                                            "Its default keys were taken from “{}”, which now has none.",
                                            b.label()
                                        )
                                    });
                                    self.shortcut_capture = None;
                                    self.save_shortcuts();
                                }
                            });
                            ui.end_row();
                        }
                    });
                });
                if let Some(n) = &self.shortcut_note {
                    ui.colored_label(palette::warn(), n);
                }
                let all_default = Action::ALL.into_iter().all(|a| self.shortcuts.is_default(a));
                if ui
                    .add_enabled(!all_default, egui::Button::new("Reset all"))
                    .clicked()
                {
                    self.shortcuts = Shortcuts::default();
                    self.shortcut_capture = None;
                    self.shortcut_note = None;
                    self.save_shortcuts();
                }
                ui.label(
                    RichText::new(
                        "Fixed: in the Words tab, ↑ ↓ Page Up/Down Home End move the selection and Ctrl+C \
                         copies the selected word; Esc in a text box leaves it (in the Library's search, clears it).",
                    )
                    .weak()
                    .small(),
                );
            });
        // Folded away while waiting for keys: stop waiting.
        if shown.body_returned.is_none() {
            self.shortcut_capture = None;
        }
    }
}

impl App {
    fn known_window(&mut self, ctx: &egui::Context) {
        let mut open = self.known.window;
        let mut changed = false;
        egui::Window::new("Known words: HSK, TOCFL & word lists")
            .id(egui::Id::unique("known-window"))
            .open(&mut open)
            .default_width(560.0)
            .show(ctx, |ui| {
                let k = &mut self.known;
                let bands = &known::BANDS;
                ui.strong("HSK 3.0 vocabulary");
                ui.label(
                    RichText::new(
                        "《HSK 考试大纲》, released November 2025 (exams from July 2026). Tick each level whose new words you know; levels 7–9 share one list.",
                    )
                    .weak(),
                );
                ui.horizontal_wrapped(|ui| {
                    let mut cum = 0;
                    for &b in bands {
                        let n = known::hsk_count(b);
                        cum += n;
                        let mut v = k.settings.band_on(b);
                        if ui
                            .checkbox(&mut v, format!("{} ({})", known::band_label(b), fmt_int(n as u64)))
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
                        let label = known::band_label(b);
                        let short = label.trim_start_matches("HSK ");
                        let first = known::band_label(bands[0]);
                        if ui.small_button(short).on_hover_text(format!("Tick {first} to {label} only")).clicked() {
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
                ui.add_space(6.0);
                ui.strong("HSK 2.0 vocabulary");
                ui.label(
                    RichText::new(
                        "The 2012 word lists (5,000 words): what the HSK tested before the 2025 syllabus, and what the HSK Standard Course textbooks teach. \
                         Tick each level whose new words you know, alongside or instead of HSK 3.0.",
                    )
                    .weak(),
                );
                ui.horizontal_wrapped(|ui| {
                    let mut cum = 0;
                    for &b in &known::HSK2_LEVELS {
                        let n = known::hsk2_count(b);
                        cum += n;
                        let mut v = k.settings.hsk2_on(b);
                        if ui
                            .checkbox(&mut v, format!("HSK {b} ({})", fmt_int(n as u64)))
                            .on_hover_text(format!(
                                "HSK 2.0 level {b}: {} words introduced at this level; {} up to and including it",
                                fmt_int(n as u64),
                                fmt_int(cum as u64)
                            ))
                            .changed()
                        {
                            k.settings.set_hsk2(b, v);
                            changed = true;
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("All up to:").weak());
                    for (i, &b) in known::HSK2_LEVELS.iter().enumerate() {
                        if ui
                            .small_button(b.to_string())
                            .on_hover_text(format!("Tick HSK 2.0 1 to {b} only"))
                            .clicked()
                        {
                            k.settings.hsk2 = known::HSK2_LEVELS[..=i].to_vec();
                            changed = true;
                        }
                    }
                    if ui.small_button("None").clicked() && !k.settings.hsk2.is_empty() {
                        k.settings.hsk2.clear();
                        changed = true;
                    }
                });
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
                            match known::read_word_list(&p) {
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
                    RichText::new(
                        "One word per line. The first item on a line that contains Chinese is used, so exports with pinyin or definitions after a tab, comma or space work; # starts a comment. \
                         Lists are copied in: import a file again to update it.",
                    )
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
                    Some(m) => {
                        let l = &m.listed;
                        ui.label(if anki_on {
                            format!(
                                "{} distinct words marked known; {} of them are not young/mature in Anki, adding {} characters.",
                                fmt_int(l.words),
                                fmt_int(l.new_words),
                                fmt_int(l.new_chars)
                            )
                        } else {
                            format!(
                                "{} distinct words marked known ({} characters). Anki is off, so only these count.",
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
                    RichText::new(format!(
                        "These words count as known (\"Word list\" in the Characters and Words tabs) alongside young and mature Anki cards, and a character is known if it is in a known word. \
                         Traditional ↔ Simplified matching follows the Anki window's option (now {}). Saved to {}.",
                        if self.anki.settings.match_across_scripts { "on" } else { "off" },
                        known::Settings::path().display()
                    ))
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

    /// True if the open book was segmented with other imported word lists
    /// than the current ones.
    fn known_resegment_due(&self) -> bool {
        self.job.is_none()
            && self
                .prep
                .as_ref()
                .is_some_and(|p| p.known_words != self.known.settings.segmenter_words())
    }
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
/// Whether a book matches the search: by its title, its file name or any
/// of `more` (the user's own Text columns, on the Books tab).
fn book_matches<'a>(
    q: &search::Query,
    title: &'a str,
    path: &Path,
    more: impl Iterator<Item = &'a str>,
) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    let more: Vec<&str> = more.collect();
    q.matches(
        std::iter::once(title)
            .chain(name.as_deref())
            .chain(more.iter().copied()),
    )
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
    palette::card()
        .show(ui, |ui| {
            ui.set_min_width(222.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).size(12.5).color(palette::weak()));
                    if let Some(f) = info {
                        info_button(ui, |ui| f(ui));
                    }
                });
                ui.add_space(2.0);
                ui.label(
                    value
                        .size(30.0)
                        .family(egui::FontFamily::Name(palette::BOLD.into()))
                        .strong(),
                );
                ui.add_space(2.0);
                ui.label(RichText::new(sub).color(palette::weak()));
            });
        })
        .response
}

/// A card with a title over `body`, filling the width it is given.
fn section_card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    palette::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(palette::bold_text(title, 16.0).color(palette::strong()));
        ui.add_space(4.0);
        body(ui);
    });
    ui.add_space(12.0);
}

/// A label, a rounded bar filled to `f` (0–1) and the percentage after it.
fn share_bar(ui: &mut egui::Ui, label: &str, f: f32) {
    ui.horizontal(|ui| {
        let h = ui.spacing().interact_size.y;
        let (r, _) = ui.allocate_exact_size(egui::vec2(64.0, h), egui::Sense::hover());
        let font = egui::TextStyle::Body.resolve(ui.style());
        let color = ui.visuals().text_color();
        ui.painter().text(
            r.left_center(),
            egui::Align2::LEFT_CENTER,
            label,
            font,
            color,
        );
        let w = (ui.available_width() - 64.0).clamp(80.0, 320.0);
        let (bar, _) = ui.allocate_exact_size(egui::vec2(w, 10.0), egui::Sense::hover());
        let c = palette::cat();
        ui.painter().rect_filled(bar, 5, c.surface0);
        if f > 0.0 {
            let mut fill = bar;
            fill.set_width((bar.width() * f.clamp(0.0, 1.0)).max(bar.height()));
            ui.painter().rect_filled(fill, 5, c.mauve);
        }
        ui.label(RichText::new(format!("{:.1}%", f * 100.0)).color(palette::weak()));
    });
}

/// The Overview's explanation of the writing-style rating.
const STYLE_HOVER: &str = "How hard the prose is to read when you know its words: sentence structure, written and classical register, rhetoric and description, what you must infer. Rated by a local AI model (Qwen3.5-4B) from four samples of 3,000 characters. The percentage compares it with the rated books in your Library: 50% is their median, and 9 in 10 of them fall between 5% and 95%. The word after it (plain, typical, literary, very literary) is the model's own judgment, the same for everyone. It counts toward the difficulty with the weight set in Settings (⚙), where it can be turned off.";

impl App {
    /// The writing-style scale fitted to the rated books of the Library
    /// ([`style_scale`]).
    fn style_scale(&self) -> StyleScale {
        let rated = self.style.store.current();
        StyleScale::fit(
            self.history
                .entries
                .iter()
                .filter_map(|e| rated.get(e.id.as_str()).copied()),
        )
    }
}

fn difficulty_color(d: Difficulty) -> Color32 {
    use palette::{MOCHA as M, ink};
    match d {
        Difficulty::Easy => palette::pick(M.green, ink::GREEN),
        Difficulty::Moderate => palette::pick(M.teal, ink::TEAL),
        Difficulty::Challenging => palette::pick(M.yellow, ink::YELLOW),
        Difficulty::Hard => palette::pick(M.peach, ink::PEACH),
        Difficulty::VeryHard => palette::pick(M.red, ink::RED),
    }
}

/// Difficulty as a fill (bars, points, bands): on the light theme Latte's
/// own accents, brighter than the text colours of [`difficulty_color`].
pub(crate) fn difficulty_fill(d: Difficulty) -> Color32 {
    let f = palette::cat();
    match d {
        Difficulty::Easy => f.green,
        Difficulty::Moderate => f.teal,
        Difficulty::Challenging => f.yellow,
        Difficulty::Hard => f.peach,
        Difficulty::VeryHard => f.red,
    }
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
        let default_cpm = self.cpm();
        let r = self.report.as_ref().unwrap();
        // Writing style, while ratings are on: (rating, hover). Shown in the
        // difficulty tile, which it counts toward; a tile of its own when
        // nothing is marked known.
        let style_info: Option<(String, String)> = self.style_on().then(|| {
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
        let image_busy = self.image_busy();
        let traditional_variant = r.variant == Variant::Traditional;
        let mut save_image = false;
        let mut read_book = false;
        let mut export_audiobook = false;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            // The title, wrapped clear of the image button at the right.
            ui.horizontal_top(|ui| {
                let w = (ui.available_width() - 160.0).max(120.0);
                ui.allocate_ui(egui::vec2(w, 0.0), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if let Some(t) = &r.title {
                            ui.heading(t);
                        }
                        if r.format == "EPUB" && self.path.is_some() {
                            read_book = ui
                                .button(RichText::new("📕 Read").size(15.0))
                                .on_hover_text("Read the book, with its layout and pictures")
                                .clicked();
                            export_audiobook = ui
                                .button(RichText::new("Export audiobook…").size(15.0))
                                .clicked();
                        }
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    save_image = ui
                        .add_enabled(!image_busy, egui::Button::new("🖼 Save as image…"))
                        .on_hover_text("Save this Overview as a picture (PNG): the cover, figures, difficulty and the most frequent unknown words")
                        .on_disabled_hover_text("An image is being saved")
                        .clicked();
                });
            });
            let (included, sections) = (self.included.iter().filter(|x| **x).count(), self.included.len());
            ui.label(format!(
                "{} · {:?} Chinese (traditional-only chars: {}, simplified-only: {}) · {} of {} sections included",
                r.format,
                r.variant,
                fmt_int(r.trad_marks),
                fmt_int(r.simp_marks),
                included,
                sections
            ));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                big_stat(ui, "Chinese characters (total)", r.han_total, &format!("{} occur only once", fmt_int(r.han_hapax as u64)));
                big_stat(ui, "Unique characters", r.han_unique as u64, &format!("{} needed for 99% coverage", r.char_coverage.iter().find(|c| c.percent == 99.0).map(|c| c.items).unwrap_or(0)));
                big_stat(ui, "Chinese words (total)", r.words_total, &format!("segmenter: {}", r.segmenter.label()));
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
                    );
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
                    let tiles: &[(&str, &anki::KnownCount)] = &[
                        ("Unique characters you know", &c.unique_chars),
                        ("Unique words you know", &c.unique_words),
                    ];
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
                    let running_text = hapax_note + &format!(
                        "Every occurrence counts, so frequent words weigh more. {} of {} word occurrences are known; {} occurrences of names (your user dictionary and the recommended Suggested words) are left out. \
                         Of the running characters, {:.1}% are known.",
                        fmt_int(rw.known),
                        fmt_int(rw.total),
                        fmt_int(c.name_tokens),
                        c.running_chars.percent()
                    );
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
                    );
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
                            RichText::new(
                                "Thresholds from L2 reading research: 98% coverage for comfortable reading of fiction (Hu & Nation 2000), 95% as the minimum for adequate comprehension (Laufer 1989). \
                                 Words you know but have no Anki card for (and aren't in a ticked HSK or TOCFL level or word list) count as unknown, so the book may be easier than this.",
                            )
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
                    );
                });
                let anki_part = format!(
                    "on a young or mature card in your selected decks ({} words, {} characters)",
                    fmt_int(c.anki.words.known()),
                    fmt_int(c.anki.chars.known()),
                );
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
                    RichText::new(format!(
                        "Known = {known_def}. A character is known if it appears in a known word. Click ⓘ on the last two tiles for details."
                    ))
                    .weak(),
                );
                ui.add_space(10.0);
            }

            ui.columns(2, |cols| {
                let ui = &mut cols[0];
                section_card(ui, "Composition", |ui| {
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
                });
                section_card(ui, "Structure", |ui| {
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
                        let default = default_cpm;
                        let mins = r.han_total as f64 / self.cpm.unwrap_or(default) as f64;
                        ui.label(format!("{}h {:02}m at", (mins / 60.0) as u64, (mins % 60.0) as u64));
                        speed_input(ui, &mut self.cpm, default, 50.0..=2000.0, 5.0, " chars/min");
                    });
                    ui.end_row();
                    if let Some(id) = self.report_id.as_deref() {
                        let cpm = self.cpm.unwrap_or(default_cpm);
                        let by_hand = self.settings.cpm_by_hand || self.cpm.is_some();
                        library_reading::book_rows(ui, id, r.han_total, cpm, by_hand);
                    }
                });
                });

                let ui = &mut cols[1];
                section_card(ui, "Coverage", |ui| {
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
                });
                section_card(ui, "Word length (share of running words)", |ui| {
                let total = r.words_total.max(1) as f32;
                for (i, n) in r.word_length_tokens.iter().enumerate() {
                    let label = if i == 4 { "5+ chars".to_owned() } else { format!("{} char{}", i + 1, if i == 0 { "" } else { "s" }) };
                    share_bar(ui, &label, *n as f32 / total);
                }
                });
            });
        });
        let ctx = ui.ctx().clone();
        if save_image {
            self.export_image(&ctx, png_report::ImageExport::Book);
        }
        if export_audiobook && let Some(path) = self.path.clone() {
            let cover = self.book.as_ref().and_then(|book| book.cover.clone());
            self.audiobook = Some(reader::Audiobook::new(
                path,
                self.report_id.as_deref(),
                traditional_variant,
                cover,
                &ctx,
            ));
        }
        if read_book && let Some(path) = self.path.clone() {
            let traditional = self
                .report
                .as_ref()
                .is_some_and(|r| r.variant == Variant::Traditional);
            match reader::Reader::open(&path, self.report_id.as_deref(), traditional, &ctx) {
                Ok(r) => self.reader = Some(r),
                Err(e) => self.set_error("reader", e),
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
                let f = ui.add(
                    egui::TextEdit::singleline(&mut self.char_filter)
                        .hint_text("characters to find")
                        .desired_width(200.0),
                );
                let f = match self.shortcuts.text(ui.ctx(), Action::Filter) {
                    Some(k) => f.on_hover_text(k),
                    None => f,
                };
                Self::focus_filter_box(&mut self.focus_filter, ui, &f, &self.char_filter);
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
                f.is_empty() || w.contains(f)
            })
            .filter(|&i| status.is_none_or(|s| key.known.matches(s[i])))
            .collect();
        let (col, desc) = key.sort;
        let word = |i: usize| r.word_freq[i].0.as_str();
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
                let f = ui.add(
                    egui::TextEdit::singleline(&mut self.word_filter)
                        .hint_text("substring")
                        .desired_width(200.0),
                );
                let f = match self.shortcuts.text(ui.ctx(), Action::Filter) {
                    Some(k) => f.on_hover_text(k),
                    None => f,
                };
                Self::focus_filter_box(&mut self.focus_filter, ui, &f, &self.word_filter);
                anki_filter_combo(ui, &mut self.anki_filter, r.anki.is_some());
                ui.label(format!(
                    "{} unique words",
                    fmt_int(r.word_freq.len() as u64)
                ));
                let mark = self.shortcuts.text(ui.ctx(), Action::MarkKnown);
                let copy = shortcuts::display(
                    ui.ctx(),
                    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::C),
                );
                ui.label(
                    RichText::new(match mark {
                        Some(m) => format!("↑ ↓ select a word · {m} marks it known or unmarks it · {copy} copies it"),
                        None => format!("↑ ↓ select a word · {copy} copies it"),
                    })
                    .weak(),
                );
            });
        }
        let rows = self.word_rows();
        let r = self.report.as_ref().unwrap();
        let status = r.anki.as_ref().map(|c| &c.word_status);
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
            let (home, end, steps, copy) = ui.input_mut(|i| {
                use egui::{Key, Modifiers};
                let mut n = |key| i.count_and_consume_key(Modifiers::NONE, key) as isize;
                let steps = n(Key::ArrowDown) - n(Key::ArrowUp)
                    + page * (n(Key::PageDown) - n(Key::PageUp));
                let (home, end) = (n(Key::Home) > 0, n(Key::End) > 0);
                // Ctrl+C arrives as a copy event, not a key.
                let before = i.events.len();
                i.events.retain(|e| !matches!(e, egui::Event::Copy));
                (home, end, steps, i.events.len() < before)
            });
            let mark = self.shortcuts.consume(ui.ctx(), Action::MarkKnown);
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
                        "{} is known from {from}; {} only unmarks words you marked",
                        w,
                        self.shortcuts
                            .text(ui.ctx(), Action::MarkKnown)
                            .unwrap_or_default()
                    ));
                } else {
                    toggle = Some((w.clone(), true));
                }
            }
            if copy && let Some(p) = pos {
                let w = word_of(p);
                ui.ctx().copy_text(w.to_owned());
                note = Some(format!("Copied {w}"));
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
                        "Length",
                        Some((WordCol::Length, true)),
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
                        ui.label(RichText::new(w).size(18.0));
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
                        ui.label(w.chars().count().to_string());
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
        let shown = w;
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
            .columns(Column::exact(80.0), 6)
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
                for t in heads {
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
                    for v in values {
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
        self.forced_words_ui(ui, ctx, true);
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
                for line in METHOD {
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
     file name: 三体* starts with 三体, *.txt ends with .txt. Case is ignored and 三體 finds 三体. On Books, your own \
     Text columns (☷ Columns…) are searched too, each word in any of them. Esc clears it.";

impl App {
    /// Every book analysed before, as recorded when its analysis (or what
    /// counts as known) last changed. A click on a title opens the book.
    fn history_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Library");
            ui.add_space(8.0);
            let items = [
                (HistTab::Books, "Books"),
                (HistTab::Path, "Reading path"),
                (HistTab::Stats, "Stats"),
            ];
            for (t, name) in items {
                let r = palette::tab(ui, self.history_tab == t, name, 14.0);
                if r.clicked() {
                    self.history_tab = t;
                }
                r.on_hover_text(match t {
                    HistTab::Books => "Every book analysed, with its statistics",
                    HistTab::Path => "An order to read the books in, where the words you study for one book make the next ones easier",
                    HistTab::Stats => "The library as a whole: its size, its vocabulary and how much of it you know",
                });
            }
            ui.add_space(8.0);
            if self.history_tab != HistTab::Stats {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.library_filter)
                        .hint_text("🔍 Search")
                        .desired_width(180.0),
                )
                .on_hover_text(self.shortcuts.hint(ctx, Action::Filter, "Search the Library (ⓘ: how)"));
                Self::focus_filter_box(&mut self.focus_filter, ui, &r, &self.library_filter);
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
                if self.history_tab == HistTab::Books {
                    self.columns_button(ui);
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
                if ui
                    .selectable_label(self.du.open, "Add from DuChinese…")
                    .on_hover_text(
                        "Browse DuChinese's graded stories and courses, see how much of each you'd know, \
                         and import them as books (premium chapters when signed in with a subscription)",
                    )
                    .clicked()
                {
                    if self.du.open {
                        self.du.open = false;
                    } else {
                        self.du_open(ctx);
                    }
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
            HistTab::Stats => return self.stats_view(ui),
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

        self.books_view(ui, ctx);
    }

    /// Copy the Library's books still read from elsewhere (added before it
    /// kept copies) into its folder, in the background, one at a time.
    fn move_books(&mut self, ctx: &egui::Context) {
        let books_dir = self.history.books();
        let todo = self.history.outside();
        if todo.is_empty() {
            std::thread::spawn(move || books::clean_incoming(&books_dir));
            return;
        }
        let (tx, rx) = channel();
        let done = Arc::new(AtomicUsize::new(0));
        let total = todo.len();
        let (d2, ctx) = (done.clone(), ctx.clone());
        std::thread::spawn(move || {
            books::clean_incoming(&books_dir);
            for (id, path) in todo {
                // Gone: nothing to copy. Changed since it was analysed: not
                // this book any more (opened again, it is a new one).
                if path.is_file() {
                    match books::import(&books_dir, &path, None) {
                        Ok(i) if i.id == id => {
                            let _ = tx.send(Ok((id, i.path)));
                        }
                        Ok(i) => {
                            if i.copied {
                                books::delete(&books_dir, &i.id);
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e));
                        }
                    }
                }
                d2.fetch_add(1, Ordering::Relaxed);
                ctx.request_repaint();
            }
        });
        self.moves = Some(BookMoves {
            rx,
            done,
            total,
            failed: Vec::new(),
        });
    }

    /// Point the Library's entries at the copies [`Self::move_books`] made.
    fn poll_moves(&mut self, ctx: &egui::Context) {
        let Some(m) = &mut self.moves else { return };
        let mut moved = Vec::new();
        let finished = loop {
            match m.rx.try_recv() {
                Ok(Ok(x)) => moved.push(x),
                Ok(Err(e)) => m.failed.push(e),
                Err(TryRecvError::Empty) => break false,
                Err(TryRecvError::Disconnected) => break true,
            }
        };
        if !moved.is_empty() {
            // A book removed meanwhile (and not to be put back) keeps no copy.
            let books_dir = self.history.books();
            for (id, _) in &moved {
                let kept = self.history.entries.iter().any(|e| e.id == *id)
                    || self.removed.as_ref().is_some_and(|r| r.entry.id == *id);
                if !kept {
                    books::delete(&books_dir, id);
                }
            }
            if let Err(e) = self.history.relocate(&moved) {
                self.history_error = Some(e);
            }
        }
        if !finished {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        let Some(m) = self.moves.take() else { return };
        if let Some(first) = m.failed.first() {
            self.set_error(
                "copy",
                format!(
                    "{} of the Library's books could not be copied into its folder; they are read where they are, and copied next time hanstat starts. {first}",
                    m.failed.len()
                ),
            );
        }
    }

    /// The Library book last read in the reader (its place moved last), for
    /// the toolbar's 📕 Reader.
    fn last_read(&self) -> Option<LastRead> {
        let entries = &self.history.entries;
        let id = reading_stats::last_read(|id| {
            entries
                .iter()
                .any(|e| e.id == id && e.stats.format == "EPUB")
        })?;
        let e = entries.iter().find(|e| e.id == id)?;
        Some(LastRead {
            id,
            path: e.path.clone(),
            title: e.title.clone(),
            traditional: e.stats.variant == "Traditional",
        })
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
            self.home_view.forget(id);
            chapter_lengths::forget(id);
            book_series::forget(id);
            self.columns.forget(id);
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
    /// The library of the books `have` (with a profile), folded as the
    /// known words need.
    fn lib_key(&self, have: Vec<String>) -> LibKey {
        LibKey {
            fold: plan::Library::fold_for(
                self.known.merged.as_deref(),
                self.anki.settings.match_across_scripts,
            ),
            skip_hapax: self.settings.skip_hapax,
            ids: have,
            version: self.profiles_version,
        }
    }

    /// History ids of the books that have a profile.
    fn plan_have(&mut self) -> Vec<String> {
        let key = (self.profiles_version, self.history.entries.len());
        if let Some((k, ids)) = &self.plan.have
            && *k == key
        {
            return ids.clone();
        }
        let ids: Vec<String> = self
            .history
            .entries
            .iter()
            .filter(|e| self.history.profile(&e.id).is_file())
            .map(|e| e.id.clone())
            .collect();
        self.plan.have = Some((key, ids.clone()));
        ids
    }

    /// History ids of the books with a profile but no [`profile::Positions`]
    /// file beside it (analysed before they were kept).
    fn plan_no_positions(&mut self) -> HashSet<String> {
        let key = (self.profiles_version, self.history.entries.len());
        if let Some((k, ids)) = &self.plan.no_positions
            && *k == key
        {
            return ids.clone();
        }
        let ids: HashSet<String> = self
            .plan_have()
            .into_iter()
            .filter(|id| !self.history.positions(id).is_file())
            .collect();
        self.plan.no_positions = Some((key, ids.clone()));
        ids
    }

    /// Plan in the background; a running plan is cancelled, and this one
    /// waits until it has stopped.
    fn start_plan(&mut self, inputs: PlanInputs, ctx: &egui::Context) {
        let prev = self.plan.job.take().map(|j| {
            j.cancel.store(true, Ordering::Relaxed);
            j.thread
        });
        self.plan.inputs = Some(inputs.clone());
        self.plan.started = ctx.input(|i| i.time);
        if inputs.books.is_empty() {
            self.plan.result = None;
            return;
        }
        let old = self.plan.lib.clone();
        let titles: HashMap<&str, &str> = self
            .history
            .entries
            .iter()
            .map(|e| (e.id.as_str(), e.title.as_str()))
            .collect();
        let books: Vec<(String, String)> = inputs
            .lib
            .ids
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    titles.get(id.as_str()).unwrap_or(&"?").to_string(),
                )
            })
            .collect();
        let k = self.known.merged.clone();
        let read_counts = self.across.counts.clone();
        let history_path = History::path();
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let thread = std::thread::spawn(move || {
            // One plan at a time: the one replaced stops at its next check.
            if let Some(h) = prev {
                let _ = h.join();
            }
            let t = std::time::Instant::now();
            let key = inputs.lib.clone();
            let Some(built) = plan_library(&key, books, &history_path, old, &c2) else {
                return;
            };
            let files = built.files;
            // Writing style: a copy only when the offsets changed (the
            // books' word data is shared, not copied).
            let mut lib = built.lib;
            let off: HashMap<&str, f64> = inputs
                .offsets
                .iter()
                .map(|(id, o)| (id.as_str(), *o))
                .collect();
            let want = |id: &str| off.get(id).copied().unwrap_or(0.0);
            if lib.books.iter().any(|b| b.offset != want(&b.id)) {
                Arc::make_mut(&mut lib).set_offsets(want);
            }
            // Series from the books' metadata, where they name one.
            let meta: HashMap<&str, &hanstat::series::Meta> = inputs
                .series
                .iter()
                .map(|(id, m)| (id.as_str(), m))
                .collect();
            let series = lib.series_with(|id| meta.get(id).map(|m| (*m).clone()));
            if lib.series_differ(&series) {
                Arc::make_mut(&mut lib).set_series(series);
            }
            // Where the reader is in each book started (learning while
            // reading): its EPUB file, the share of that file's text before
            // the place (as the reader counts it), then a running word.
            let at: HashMap<&str, (u64, u64)> = inputs
                .reading
                .iter()
                .filter_map(|(id, path, spine, offset, saved)| {
                    if c2.load(Ordering::Relaxed) {
                        return None;
                    }
                    let b = lib.books.iter().find(|b| b.id == *id)?;
                    if !b.has_positions() {
                        return None;
                    }
                    let epub = reader::epub::Epub::open(path).ok()?;
                    let href = epub.spine.get(*spine as usize)?;
                    let len = reader::chapter_text_len(&epub, href).max(1);
                    let f = *offset as f64 / len as f64;
                    Some((id.as_str(), (b.running_word_at(href, f)?, *saved)))
                })
                .collect();
            if c2.load(Ordering::Relaxed) {
                return;
            }
            let at = |b: &plan::LibBook| at.get(b.id.as_str()).copied();
            if lib.reading_differs(at) {
                Arc::make_mut(&mut lib).set_reading(at);
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
            let mut k0 = lib.known(k.as_deref());
            let params = inputs.params;
            // Learning while reading, a word's meetings in the books read count.
            if matches!(params.study, plan::Study::Read { .. }) {
                lib.add_read_meetings(&mut k0, read_counts.values().map(|c| &c[..]));
            }
            let width = plan::width_for(&lib, &books, plan::SEARCH_BUDGET);
            let Some(best) = lib.optimise(&k0, &params, &books, width, &c2) else {
                return;
            };
            let naive = lib.evaluate(
                &k0,
                &params,
                &lib.started_first(&params, &lib.easiest_now(&k0, &books)),
            );
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
                            // Keeping series together, a volume goes after its series.
                            match params
                                .keep_series
                                .then(|| o.iter().rposition(|&x| lib.same_series(x, s.book)))
                                .flatten()
                            {
                                Some(i) => o.insert(i + 1, s.book),
                                None => o.push(s.book),
                            }
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
                files,
                path,
                naive,
                optimised,
                secs: t.elapsed().as_secs_f64(),
            });
            ctx.request_repaint();
        });
        self.plan.job = Some(PlanJob { rx, cancel, thread });
    }

    fn poll_plan(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.plan.job else {
            return;
        };
        match job.rx.try_recv() {
            Ok(d) => {
                self.plan.lib = Some(PlanLib {
                    key: d.key.clone(),
                    lib: d.lib.clone(),
                    files: d.files.clone(),
                });
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
        self.home = false;
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

    /// What a Library row is computed from besides its book, its sections
    /// and its segmenter: the build, the user dictionary, the known words
    /// (for the segmenter and for coverage) and the settings, as a digest.
    /// `None` while the Anki cards are still being read, when a row could not
    /// be computed right.
    fn lang_inputs(&mut self) -> Option<Inputs> {
        use sha2::{Digest, Sha256};
        let (user_words, anki, known) = (&self.user_words, &self.anki, &self.known);
        let anki_pending = anki.rx.is_some()
            || anki.dirty
            || (anki.settings.enabled
                && anki.settings.collection.is_some()
                && anki.knowledge.is_none());
        if anki_pending {
            return None;
        }
        // Character folding changes a book's character counts.
        let fold = anki.settings.match_across_scripts;
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
        if let Some((q, m, i)) = &self.inputs_memo
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
        h.update([u8::from(fold)]);
        h.update((user_words.len() as u64).to_le_bytes());
        for w in user_words {
            text(&mut h, w);
        }
        // Word lists join the segmenter's tokens.
        let seg = known.settings.segmenter_words();
        h.update((seg.len() as u64).to_le_bytes());
        for w in &seg {
            text(&mut h, w);
        }
        let analysis = hex(h);
        let mut h = Sha256::new();
        text(&mut h, env!("HANSTAT_COVERAGE_FINGERPRINT"));
        h.update([u8::from(self.settings.skip_hapax)]);
        text(&mut h, &knowledge);
        let inputs = Inputs {
            analysis,
            coverage: hex(h),
        };
        self.inputs_memo = Some((quick, known.merged.clone(), inputs.clone()));
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
        let inputs = self.lang_inputs();
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
        if self.batch.is_some() || self.job.is_some() {
            return;
        }
        let Some(d) = inputs else {
            return;
        };
        let wait = self.auto.changed_at + AUTO_REFRESH_DELAY - now;
        if wait > 0.0 {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.min(1.0)));
            return;
        }
        if self.auto.paused.as_ref() == Some(&d) {
            return;
        }
        self.auto.paused = None;
        let no_positions = self.plan_no_positions();
        let open = self.book_id.as_deref();
        // Rows of which only the known words changed first: they take
        // milliseconds each, the others an analysis.
        let (mut todo, mut full) = (Vec::new(), Vec::new());
        for e in &self.history.entries {
            if Some(e.id.as_str()) == open || self.auto.failed.get(&e.path) == Some(&d) {
                continue;
            }
            if e.inputs == d.joined() {
                // Analysed before word positions were kept: again, which
                // mostly reads the analysis cache.
                if no_positions.contains(&e.id) && e.path.is_file() {
                    full.push(Self::library_item(e));
                }
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
        // The dictionary, known words for the segmenter, and knowledge for
        // the History's "Known".
        let words = self.user_words.clone();
        let known_words = self.known.settings.segmenter_words();
        let knowledge = self.known.merged.clone();
        let inputs = self.lang_inputs();
        let thread_inputs = inputs.clone();
        let cache = self.cache.clone();
        let skip_hapax = self.settings.skip_hapax;
        let fold_chars = self.fold_chars();
        let history_path = History::path();
        let books_dir = self.history.books();
        // Books opened again keep the sections chosen for them, found by id
        // (the entry's path is the Library's copy, not the file opened).
        let saved_sections: HashMap<String, Vec<bool>> = if mode == BatchMode::Open {
            self.history
                .entries
                .iter()
                .filter_map(|e| e.sections.clone().map(|s| (e.id.clone(), s)))
                .collect()
        } else {
            HashMap::new()
        };
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
                let sections = sections.or_else(|| {
                    if saved_sections.is_empty() {
                        return None;
                    }
                    let id = history::file_id(&path).ok()?;
                    saved_sections.get(&id).cloned()
                });
                if let Some(e) = refresh.and_then(|e| {
                    coverage_only(
                        &e,
                        knowledge.as_deref(),
                        skip_hapax,
                        &history_path,
                        thread_inputs.as_ref(),
                    )
                }) {
                    let _ = tx.send(BatchMsg::Book(Box::new(e), false));
                    d2.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let res = profile::analyse_file(
                    &path,
                    kind,
                    sections.as_deref(),
                    &words,
                    &known_words,
                    cache.as_ref(),
                    fold_chars,
                )
                .and_then(|a| {
                    let id = a
                        .file_id
                        .ok_or_else(|| format!("cannot read {}", path.display()))?;
                    let mut r = a.report;
                    let names: HashSet<String> = a.names.into_iter().collect();
                    if let Some(k) = &knowledge {
                        r.anki = Some(BookCoverage::compute(
                            k,
                            &r.char_freq,
                            &r.word_freq,
                            &names,
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
                    let mut changed =
                        Profile::load(&p_path).map(|o| o.digest()) != Some(p.digest());
                    if changed {
                        p.save(&p_path)?;
                    }
                    // Where its words occur, for the reading path; read only
                    // with the profile they were made with.
                    let pos_path = profile::positions_path_in(&history_path, &id);
                    if changed
                        || profile::Positions::load(&pos_path, p.digest(), p.words.len()).is_none()
                    {
                        a.positions.save(&pos_path, p.digest())?;
                        // The reading path reads them.
                        changed = true;
                    }
                    // A book opened is copied into the Library's folder; the
                    // Library's own books are copied there at start
                    // (`App::move_books`).
                    let mut stored = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
                    if mode == BatchMode::Open {
                        match books::import(&books_dir, &path, Some(&id)) {
                            Ok(i) => stored = i.path,
                            Err(e) => {
                                let _ = tx.send(BatchMsg::CopyFailed(e));
                            }
                        }
                    }
                    Ok((
                        history::Entry {
                            id,
                            path: stored,
                            title: r
                                .title
                                .clone()
                                .filter(|t| !t.trim().is_empty())
                                .unwrap_or_else(|| file_stem(&path)),
                            analysed: history::now(),
                            sections: Some(r.sections.iter().map(|s| s.included).collect()),
                            inputs: thread_inputs
                                .as_ref()
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
        let mut copy_failed = None;
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
                            .any(|x| x.path == path)
                            .then(|| b.inputs.clone())
                            .flatten()
                    {
                        self.auto.failed.insert(path, d);
                    }
                    b.failed.push(format!("{label}: {e}"));
                }
                BatchMsg::CopyFailed(e) => copy_failed = Some(e),
                BatchMsg::Finished => finished = true,
            }
        }
        if let Some(e) = copy_failed {
            self.set_error("copy", e);
        }
        if !finished {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
            return;
        }
        let b = self.batch.take().unwrap();
        let stopped = b.cancel.load(Ordering::Relaxed);
        if b.mode == BatchMode::Auto && stopped {
            // Stopped by the user: not again until something changes.
            self.auto.paused = b.inputs.clone();
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

    /// The reading path of the History's books.
    fn plan_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut changed = false;
        let style_on = self.style_on();
        let st = &mut self.plan.settings;
        let read_mode = st.mode == plan::Mode::Read;
        ui.horizontal_wrapped(|ui| {
            changed |= ui
                .radio_value(&mut st.mode, plan::Mode::Read, "Learn while reading")
                .on_hover_text(
                    "Study nothing before a book: each book teaches the words it uses often, \
                     and the books are ordered so that each starts as easy as it can",
                )
                .changed();
            changed |= ui
                .radio_value(&mut st.mode, plan::Mode::Study, "Study before each book")
                .on_hover_text("Before each book, study its unknown words until enough of it is known")
                .changed();
            ui.separator();
            ui.label("Best order:");
            let (peak, total, peak_hover, total_hover) = match (st.mode, st.fixed) {
                (plan::Mode::Read, _) => (
                    "Easiest beginning",
                    "Fewest unknown words",
                    "Every book as easy to get into as it can be: the order in which even the book with the hardest \
                     beginning (its first few chapters) has as many of its words known as possible",
                    "The order in which you meet the fewest unknown words over all the books. \
                     It can put a long book early, even if it is harder to get into, \
                     because the words it teaches make the books after it easier",
                ),
                (plan::Mode::Study, false) => (
                    "Fewest before any book",
                    "Fewest in all",
                    "The order in which the book that needs the most study before it needs as few words as possible",
                    "The order in which you study the fewest words over all the books. \
                     It can put a long book early, because what you study for it also helps with the books after it",
                ),
                (plan::Mode::Study, true) => (
                    "Easiest hardest book",
                    "Easiest on average",
                    "The order in which the book you know least of while reading it is as easy as possible",
                    "The order in which you know the most of each book while reading it, on average",
                ),
            };
            changed |= ui
                .radio_value(&mut st.objective, plan::Objective::Peak, peak)
                .on_hover_text(peak_hover)
                .changed();
            changed |= ui
                .radio_value(&mut st.objective, plan::Objective::Total, total)
                .on_hover_text(total_hover)
                .changed();
            ui.separator();
            changed |= ui
                .checkbox(&mut st.keep_series, "Keep series together")
                .on_hover_text(
                    "Read all the books of a series one after another, in order (volume 1, 2, 3…), \
                     with no other book between them. The whole series goes where it is best on the path, \
                     counting what the earlier volumes teach for the later ones. \
                     An order you set by hand is kept until you click Use the planner's order. \
                     Off: other books can come between volumes, though volumes are still read in order",
                )
                .changed();
        });
        if read_mode {
            ui.horizontal_wrapped(|ui| {
                let hover = "The rule ✔ Mark read uses (one setting: changing it here changes the Mark read default too). \
                             A word none of the books teaches on its own is learned once met this many times in two or more books.";
                ui.label("A book teaches the").on_hover_text(hover);
                changed |= ui
                    .add(egui::DragValue::new(&mut st.read_percent).range(1..=100).suffix("%"))
                    .on_hover_text(hover)
                    .changed();
                ui.label("most frequent of the words it uses at least").on_hover_text(hover);
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut st.read_min_count)
                            .range(1..=10_000)
                            .speed(0.2),
                    )
                    .on_hover_text(hover)
                    .changed();
                ui.label("times; words met that often across books are learned too (as ✔ Mark read).")
                    .on_hover_text(hover);
            });
        }
        if !read_mode {
            ui.horizontal_wrapped(|ui| {
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
        }
        if changed && let Err(e) = st.save() {
            self.plan.error = Some(e);
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
            .filter(|e| !have_set.contains(e.id.as_str()))
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
            manual: st.manual_order().cloned(),
            offsets,
            // Not in the other mode, where places don't count: no replanning
            // after each page read.
            reading: if read_mode {
                have.iter()
                    .filter(|id| self.known.settings.read_book(id).is_none())
                    .filter_map(|id| {
                        let at = reading_stats::left_at(id)?;
                        let e = self.history.entries.iter().find(|e| e.id == *id)?;
                        Some((id.clone(), e.path.clone(), at.spine, at.offset, at.saved))
                    })
                    .collect()
            } else {
                Vec::new()
            },
            read_counts: if read_mode {
                Arc::as_ptr(&self.across.counts) as *const () as usize
            } else {
                0
            },
            series: {
                let all = book_series::all();
                have.iter()
                    .filter_map(|id| Some((id.clone(), all.get(id)?.clone())))
                    .collect()
            },
        };
        if self.plan.inputs.as_ref() != Some(&inputs) {
            // While books are being analysed, only their word data changing:
            // the plan running finishes, and the next waits a little.
            let now = ctx.input(|i| i.time);
            let wait = self.plan.started + REPLAN_SECS - now;
            let hold = self.batch.is_some()
                && self
                    .plan
                    .inputs
                    .as_ref()
                    .is_some_and(|i| i.same_but_word_data(&inputs))
                && (self.plan.job.is_some() || wait > 0.0);
            if hold {
                if wait > 0.0 {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
                }
            } else {
                self.start_plan(inputs, ctx);
            }
        }
        let entries: Vec<&history::Entry> = self.history.entries.iter().collect();

        let mut act: Option<PlanAction> = None;
        // Books without word data.
        if self.batch.is_some() {
            ui.horizontal(|ui| self.batch_status(ui));
        } else if !missing.is_empty() {
            let can = missing.iter().filter(|(_, p)| self.exists.get(p)).count();
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "{} of the {} books have no word data yet (analysed by an older version) and are not on the path.",
                    missing.len(),
                    entries.len()
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
        let reading_mode = matches!(path.params.study, plan::Study::Read { .. });
        let total_first = path.params.objective == plan::Objective::Total;
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
        // The two figures in the order the objective compares them.
        let summary = |p: &plan::Path| {
            let (peak, total) = if reading_mode {
                (
                    format!("hardest beginning {:.1}% known", p.lowest_start()),
                    format!(
                        "{} unknown words met in all, {} learned",
                        fmt_int(p.total_lookups()),
                        fmt_int(p.total_learned() as u64)
                    ),
                )
            } else if fixed {
                (
                    format!("lowest coverage while reading {:.1}%", p.lowest_reading()),
                    format!(
                        "average {:.1}%",
                        p.steps.iter().map(|s| s.reading).sum::<f64>()
                            / p.steps.len().max(1) as f64
                    ),
                )
            } else {
                (
                    format!("at most {} words before one book", fmt_int(p.peak() as u64)),
                    format!("{} words to study in all", fmt_int(p.total_study() as u64)),
                )
            };
            if total_first {
                format!("{total}, {peak}")
            } else {
                format!("{peak}, {total}")
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
        // Learning while reading, books analysed before word positions were kept.
        let estimated = if reading_mode {
            books
                .iter()
                .filter(|&&b| !lib.books[b].has_positions())
                .count()
        } else {
            0
        };
        if estimated > 0 {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} {} an estimate of where {} words occur until analysed again (the Library does this in the background).",
                        fmt_int(estimated as u64),
                        if estimated == 1 { "book uses" } else { "books use" },
                        if estimated == 1 { "its" } else { "their" },
                    ))
                    .weak(),
                );
                if self.batch.is_none()
                    && ui
                        .small_button("Analyse now")
                        .on_hover_text("Analyse them again in the background; saved analyses make this quick")
                        .clicked()
                {
                    act = Some(PlanAction::Backfill);
                }
            });
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
                        Some(e) => book_matches(&query, &b.title, &e.path, std::iter::empty()),
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
        // The other columns: #, the writing style, six figures, and the
        // buttons' least width.
        let others = 88.0 + 96.0 * 6.0 + 96.0 + if style_on { 130.0 } else { 0.0 };
        let book_w = title_column_width(
            ui,
            // Every step, not the search's: the columns stay put while typing.
            path.steps.iter().map(|s| lib.books[s.book].title.as_str()),
            others,
            8 + style_on as usize,
        );
        TableBuilder::new(ui)
            .id_salt("plan_table")
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .striped(true)
            .max_scroll_height(table_h)
            .column(Column::exact(88.0))
            .column(Column::exact(book_w).clip(true))
            .columns(Column::exact(130.0), style_on as usize)
            .columns(Column::exact(96.0), 6)
            .column(Column::remainder())
            .header(24.0, |mut h| {
                let mut heads: Vec<(&str, &str)> = if reading_mode { vec![
                    ("#", "Reading order. ⏶⏷ set your own order; the planner's is shown alongside. A book you have started comes first (📖)"),
                    ("Book", "Click a title to open the book"),
                    ("Known now", "Running words you know now (names left out)"),
                    ("When reached", "Running words known when the path reaches the book, before reading it"),
                    ("Beginning", "How much of its first few chapters you'd know (the next few, in a book you have started), counting the words it teaches as you meet them"),
                    ("Over the book", "Running words known over the whole book (the rest of it, in a book started), as its frequent words are learned"),
                    ("Teaches", "Words reading it teaches: the ones it uses often enough, and those met often enough across books"),
                    ("From earlier", "Words of this book you don't know now that the path teaches before it"),
                    ("", ""),
                ] } else { vec![
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
                ] };
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
                        // A book started: where the reader is.
                        if let Some(f) = s.from {
                            ui.label(RichText::new(format!("📖 {:.0}%", f * 100.0)).strong())
                                .on_hover_text("You are reading it: it comes first, and its figures are for the rest of it");
                        }
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
                                        plan::Study::Read { .. } => format!(
                                            "Counts as starting at {:.1}% ({:+.1} points for its writing style)",
                                            s.start - s.offset,
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
                    if reading_mode {
                        let estimate = if b.has_positions() {
                            ""
                        } else {
                            "\n\nAn estimate: where its words occur isn't known until it is analysed again."
                        };
                        row.col(|ui| {
                            // Coloured by the difficulty, writing style included.
                            let d = Difficulty::from_percent(s.start - s.offset);
                            ui.label(RichText::new(format!("{:.1}%", s.start)).color(difficulty_color(d)))
                                .on_hover_text(format!("{}: {}{estimate}", d.label(), d.description()));
                        });
                        row.col(|ui| {
                            ui.label(format!("{:.1}%", s.reading));
                        });
                        row.col(|ui| {
                            let t = ui.label(RichText::new(fmt_int(s.learned.len() as u64)).strong());
                            let left = (100.0 - s.reading) / 100.0;
                            let per1k = if left > 0.0 { left * 1000.0 } else { 0.0 };
                            let mut hover = format!(
                                "{} unknown words met reading it{}: {:.0} per 1,000 running words",
                                fmt_int(s.lookups),
                                if s.from.is_some() { " (the rest of it)" } else { "" },
                                per1k,
                            );
                            if s.from.is_some() {
                                hover.push_str(&format!(
                                    "\n{} of the words it teaches met often enough already, where you are",
                                    fmt_int(s.learned_so_far as u64)
                                ));
                            }
                            t.on_hover_text(hover);
                        });
                        row.col(|ui| {
                            ui.label(fmt_int(s.gained as u64));
                        });
                    } else {
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
                    }
                    row.col(|ui| {
                        let (any, hover) = if reading_mode {
                            (
                                !s.learned.is_empty(),
                                "Save the words it teaches (word, tab, count in the book): study them first, or make Anki cards from them while reading",
                            )
                        } else {
                            (
                                !s.study.is_empty(),
                                "Save the words to study before it (word, tab, count in the book): import it as a word list once learned, or make Anki cards from it",
                            )
                        };
                        if ui
                            .add_enabled(any, egui::Button::new("💾").small())
                            .on_hover_text(hover)
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
                st.set_manual_order(ids);
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
                st.clear_manual_order();
            }
            PlanAction::SaveList(i) => {
                save_settings = false;
                let Some(res) = &self.plan.result else { return };
                let s = &res.path.steps[i];
                let title = &res.lib.books[s.book].title;
                let teaches = matches!(res.path.params.study, plan::Study::Read { .. });
                let mut text = if teaches {
                    format!(
                        "# Words {title} teaches (step {} of the reading path), with their counts in the book\n",
                        i + 1
                    )
                } else {
                    format!(
                        "# Words to study before {title} (step {} of the reading path), with their counts in the book\n",
                        i + 1
                    )
                };
                let words = if teaches { &s.learned } else { &s.study };
                for (w, n) in res.lib.study_list(s.book, words) {
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
                let file = if teaches {
                    "words it teaches"
                } else {
                    "study list"
                };
                if let Some(m) = save(&format!("{} - {file}.txt", name.trim()), "txt", text) {
                    self.toast = Some((m, ui.input(|i| i.time) + 3.0));
                }
            }
            PlanAction::Backfill => {
                save_settings = false;
                let have = self.plan_have();
                let no_positions = self.plan_no_positions();
                let todo: Vec<BatchItem> = self
                    .history
                    .entries
                    .iter()
                    .filter(|e| !have.contains(&e.id) || no_positions.contains(&e.id))
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
/// The reading speed estimates use: measured in the reader unless set by
/// hand (see [`App::cpm`]).
impl App {
    /// Chinese characters a minute: the speed measured in the reader, or
    /// the one in Settings when it is set by hand or nothing is measured yet.
    pub(crate) fn cpm(&self) -> f32 {
        if self.settings.cpm_by_hand {
            return self.settings.cpm;
        }
        reading_stats::measured_now().map_or(self.settings.cpm, |v| v as f32)
    }

    /// Characters a minute, for [`library_stats::reading_minutes`].
    pub(crate) fn speeds(&self) -> f64 {
        self.cpm() as f64
    }
}

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
        .on_hover_text("This book only, to try another speed. The default is your speed measured in the reader, or the one set in Settings (⚙)")
        .changed()
    {
        *speed = Some(v);
    }
    if speed.is_some_and(|s| s != default)
        && ui
            .small_button("↺")
            .on_hover_text(format!("Back to the default, {default:.0}{suffix}"))
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
    knowledge: Option<&anki::Knowledge>,
    skip_hapax: bool,
    history_path: &Path,
    inputs: Option<&Inputs>,
) -> Option<history::Entry> {
    let inputs = inputs?;
    let p = Profile::load(&profile::path_in(history_path, &e.id))?;
    let chars = p.chars.as_ref()?;
    let names: HashSet<String> = p.names.iter().cloned().collect();
    let known = knowledge.map(|k| {
        history::Known::from_coverage(&BookCoverage::compute(
            k, chars, &p.words, &names, skip_hapax,
        ))
    });
    let mut e = e.clone();
    e.stats.known = known;
    e.inputs = inputs.joined();
    Some(e)
}

/// The segmenter a Library row names (`SegmenterKind::label`); `Auto` if none does.
fn segmenter_by_label(label: &str) -> SegmenterKind {
    [
        SegmenterKind::Ckip,
        SegmenterKind::Ltp,
        SegmenterKind::Jieba,
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
    h.update([u8::from(k.fold), u8::from(k.from_anki)]);
    h.update(k.listed.source.as_bytes());
    h.finalize()[..12]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Width of a Library table's Book column (a cover, then the title): as
/// wide as the longest of `titles`, but no wider than the `n` other columns,
/// `others` wide together, leave, and at least 140 points (longer titles
/// are cut with "…").
fn title_column_width<'a>(
    ui: &egui::Ui,
    titles: impl Iterator<Item = &'a str>,
    others: f32,
    n: usize,
) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let p = ui.painter();
    let longest = titles
        .map(|t| {
            p.layout_no_wrap(t.to_owned(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
        .fold(0.0, f32::max);
    let gap = ui.spacing().item_spacing.x;
    // The cover, 6 points, and the spacing either side of them; a little
    // slack so the last letter isn't cut.
    let want = COVER_SIZE.0 + 6.0 + 2.0 * gap + longest + 4.0;
    // Between the columns, and room for the scroll bar.
    let room = ui.available_width() - others - gap * n as f32 - 12.0;
    want.min(room).max(140.0)
}

/// Size cover thumbnails are drawn at in the History table (points), and its row height.
const COVER_SIZE: (f32, f32) = (32.0, 44.0);
const COVER_ROW: f32 = 50.0;
/// The Library's 📕 Read column.
const READ_COL: f32 = 76.0;

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
