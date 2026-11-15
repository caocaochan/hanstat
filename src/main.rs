#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use hanstat::analyze::{self, Prepared, Report};
use hanstat::anki::{self, BookCoverage, ByStatus, Status};
use hanstat::discover::{self, Candidate};
use hanstat::extract::{self, Book, EncodingChoice, SectionKind};
use hanstat::segment::SegmenterKind;
use hanstat::update::{self, Release};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

fn main() -> eframe::Result {
    // hanstat [file] [--tab overview|characters|words|sections|notes]
    let mut path = None;
    let mut tab = Tab::Overview;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--tab" {
            tab = match args.next().as_deref() {
                Some("characters") => Tab::Characters,
                Some("words") => Tab::Words,
                Some("sections") => Tab::Sections,
                Some("notes") => Tab::Notes,
                Some("suggestions") => Tab::Suggestions,
                _ => Tab::Overview,
            };
        } else {
            path = Some(PathBuf::from(a));
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("hanstat")
            .with_inner_size([1180.0, 800.0])
            .with_min_inner_size([760.0, 520.0])
            .with_drag_and_drop(true)
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
        ..Default::default()
    };
    eframe::run_native(
        "hanstat",
        options,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            let mut app = App {
                tab,
                ..App::default()
            };
            app.models = fetch_models(&cc.egui_ctx);
            app.updater.start(&cc.egui_ctx);
            app.anki_init(&cc.egui_ctx);
            if let Some(p) = path {
                app.start(p, cc.egui_ctx.clone());
            }
            Ok(Box::new(app))
        }),
    )
}

/// egui ships no CJK glyphs; use Windows system fonts as fallbacks.
fn install_fonts(ctx: &egui::Context) {
    let dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts");
    let candidates = [
        ("yahei", "msyh.ttc"),             // Microsoft YaHei: BMP CJK
        ("jhenghei", "msjh.ttc"),          // JhengHei: extra Traditional coverage
        ("simsun-extb", "simsunb.ttf"),    // SimSun-ExtB: Ext B (SIP)
        ("simsun-extg", "SimsunExtG.ttf"), // Ext G
        ("segoe-symbol", "seguisym.ttf"),
    ];
    let mut fonts = egui::FontDefinitions::default();
    for (name, file) in candidates {
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            fonts
                .font_data
                .insert(name.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().push(name.to_owned());
            }
        }
    }
    ctx.set_fonts(fonts);
}

#[derive(PartialEq, Eq, Clone, Copy, Default)]
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
        if let Some(dir) = update::exe_path().ok().and_then(Path::parent) {
            let dir = dir.to_owned();
            std::thread::spawn(move || update::clean_up(&dir));
        }
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
        .and_then(|zip| {
            let exe = update::exe_path()?;
            let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else {
                return Err(format!("Unexpected exe path {}", exe.display()));
            };
            update::install(&zip, dir, &name.to_string_lossy())
        });
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
}

struct App {
    path: Option<PathBuf>,
    segmenter: SegmenterKind,
    encoding: EncodingChoice,
    dict_path: Option<PathBuf>,
    user_words: Vec<String>,

    book: Option<Arc<Book>>,
    prep: Option<Arc<Prepared>>,
    included: Vec<bool>,
    report: Option<Report>,
    char_cum: Vec<f64>,
    word_cum: Vec<f64>,

    job: Option<Job>,
    error: Option<String>,
    tab: Tab,
    char_filter: String,
    word_filter: String,
    cpm: f32,
    toast: Option<(String, f64)>,

    suggestions: Option<Vec<Candidate>>,
    suggest_rx: Option<Receiver<Vec<Candidate>>>,
    selected: HashSet<String>,

    anki: AnkiState,
    anki_filter: AnkiFilter,
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
}

enum AnkiMsg {
    Opened(Result<anki::Collection, String>),
    Built(Result<anki::Knowledge, String>),
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

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum AnkiFilter {
    #[default]
    All,
    NotKnown,
    Known,
    Is(Status),
}

impl AnkiFilter {
    const OPTIONS: [AnkiFilter; 8] = [
        AnkiFilter::All,
        AnkiFilter::NotKnown,
        AnkiFilter::Known,
        AnkiFilter::Is(Status::Mature),
        AnkiFilter::Is(Status::Young),
        AnkiFilter::Is(Status::Learning),
        AnkiFilter::Is(Status::New),
        AnkiFilter::Is(Status::NotInAnki),
    ];
    fn label(self) -> &'static str {
        match self {
            AnkiFilter::All => "All",
            AnkiFilter::NotKnown => "Not young/mature",
            AnkiFilter::Known => "Young or mature",
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

fn status_colour(s: Status) -> Color32 {
    match s {
        Status::Mature => Color32::from_rgb(80, 170, 100),
        Status::Young => Color32::from_rgb(140, 200, 120),
        Status::Learning => Color32::from_rgb(220, 170, 60),
        Status::New => Color32::from_rgb(100, 150, 220),
        Status::NotInAnki => Color32::GRAY,
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
            dict_path: None,
            user_words: Vec::new(),
            book: None,
            prep: None,
            included: Vec::new(),
            report: None,
            char_cum: Vec::new(),
            word_cum: Vec::new(),
            job: None,
            error: None,
            tab: Tab::Overview,
            char_filter: String::new(),
            word_filter: String::new(),
            cpm: 300.0,
            toast: None,
            suggestions: None,
            suggest_rx: None,
            selected: HashSet::new(),
            anki: AnkiState::default(),
            anki_filter: AnkiFilter::All,
            fit_pending: false,
            models: None,
            fit_until: 0.0,
            fit_requested: 0.0,
            updater: Updater::default(),
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
        self.path = Some(path.clone());
        self.error = None;
        let (tx, rx) = channel();
        let done = Arc::new(AtomicUsize::new(0));
        let total = Arc::new(AtomicUsize::new(0));
        let stage = Arc::new(Mutex::new("Reading file…".to_owned()));
        let (d2, t2, s2) = (done.clone(), total.clone(), stage.clone());
        let (kind, enc, words) = (self.segmenter, self.encoding, self.user_words.clone());
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
                let prep = analyze::analyze_book(&book, kind, &words, &|d, _| {
                    d2.fetch_max(d, Ordering::Relaxed);
                    c2.request_repaint_after(std::time::Duration::from_millis(100));
                })?;
                Ok(Loaded {
                    book,
                    prep: Arc::new(prep),
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
        });
    }

    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
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
            let r = analyze::build_report(b, p, &self.included);
            self.char_cum = cumulative(r.char_freq.iter().map(|x| x.1), r.han_total);
            self.word_cum = cumulative(r.word_freq.iter().map(|x| x.1), r.words_total);
            // Word discovery depends on the included sections and the segmenter's output.
            let (tx, rx) = channel();
            let (book, included, word_freq) =
                (b.clone(), self.included.clone(), r.word_freq.clone());
            let known: HashSet<String> = self.user_words.iter().cloned().collect();
            let opts = discover::Options::for_size(r.han_total);
            std::thread::spawn(move || {
                let _ = tx.send(discover::discover(
                    &book, &included, &word_freq, &known, &opts,
                ));
            });
            self.suggest_rx = Some(rx);
            self.report = Some(r);
            self.refresh_anki_coverage();
        }
    }

    // ----- Anki -----

    fn anki_init(&mut self, ctx: &egui::Context) {
        let (s, warn) = anki::Settings::load();
        self.anki.settings = s;
        self.anki.error = warn;
        self.anki.profiles = anki::find_profiles();
        if self.anki.settings.enabled && self.anki.settings.collection.is_some() {
            self.anki_open(ctx);
        }
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
            let _ = tx.send(AnkiMsg::Built(col.knowledge(&s, &seg)));
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
        self.refresh_anki_coverage();
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
            AnkiMsg::Built(Ok(k)) => {
                self.anki.knowledge = Some(Arc::new(k));
                self.refresh_anki_coverage();
                if std::mem::take(&mut self.anki.dirty) {
                    self.anki_rebuild(ctx);
                }
            }
            AnkiMsg::Opened(Err(e)) | AnkiMsg::Built(Err(e)) => self.anki.error = Some(e),
        }
    }

    fn refresh_anki_coverage(&mut self) {
        let Some(r) = &mut self.report else { return };
        let had = r.anki.is_some();
        r.anki = match &self.anki.knowledge {
            Some(k) if self.anki.settings.enabled => {
                Some(BookCoverage::compute(k, &r.char_freq, &r.word_freq))
            }
            _ => None,
        };
        if r.anki.is_none() {
            self.anki_filter = AnkiFilter::All;
        } else if !had {
            // The Anki section just appeared on the Overview.
            self.fit_pending = true;
        }
    }

    fn anki_window(&mut self, ctx: &egui::Context) {
        let mut open = self.anki.window;
        let mut changed = false;
        let mut reopen = false;
        egui::Window::new("Anki")
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
                    ui.colored_label(Color32::from_rgb(220, 80, 80), format!("⚠ {e}"));
                }
                if a.profiles.is_empty() && a.settings.collection.is_none() {
                    ui.label("No Anki profiles found in %APPDATA%\\Anki2. Use Browse… to pick a collection.anki2.");
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
                        .on_hover_text("On: a suspended card counts by its interval. Off: suspended cards are ignored.")
                        .changed();
                    changed |= ui
                        .checkbox(&mut a.settings.match_across_scripts, "Match Traditional ↔ Simplified")
                        .on_hover_text("Fold both your cards and the book to Simplified (character level) before matching.")
                        .changed();
                });
                if !a.settings.has_selection() {
                    ui.label(RichText::new("Select at least one deck and one field.").color(Color32::from_rgb(220, 170, 60)));
                } else if let Some(k) = &a.knowledge {
                    let s = &k.summary;
                    ui.add_space(4.0);
                    ui.label(format!(
                        "{} notes{}{}{}",
                        fmt_int(s.notes),
                        if s.notes_without_chinese > 0 {
                            format!(" ({} have no Chinese in the ticked fields)", fmt_int(s.notes_without_chinese))
                        } else {
                            String::new()
                        },
                        if s.suspended_excluded > 0 {
                            format!(", {} suspended cards ignored", fmt_int(s.suspended_excluded))
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
                        "Young = review interval under {0} days, mature = {0}+ days (Anki's definitions). Learning and relearning cards are not counted as known. \
                         A ticked field holding a word counts as that word; one holding a sentence is split into words with the book's segmenter. HTML, sound tags and [readings] are ignored. \
                         The collection is read from a temporary copy and never modified. Choices are saved to {1}.",
                        anki::MATURE_IVL,
                        anki::Settings::path().display()
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
        self.user_words = words;
        self.rerun(ctx, false);
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
                let label = RichText::new(format!("⟳ Restart to use {}", rel.tag))
                    .color(Color32::from_rgb(80, 170, 100));
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
                ui.colored_label(Color32::from_rgb(220, 170, 60), "⚠ Update failed")
                    .on_hover_text(e);
            }
            UpdatePhase::Idle | UpdatePhase::Checking | UpdatePhase::UpToDate => {}
        }
    }

    /// The version label at the right of the status bar, with update settings.
    fn version_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.menu_button(
            RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).weak(),
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
                        self.error = Some(e);
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
                            ui.colored_label(Color32::from_rgb(220, 170, 60), e);
                        }
                    }
                }
                ui.separator();
                ui.hyperlink_to("Releases on GitHub", update::RELEASES_PAGE);
            },
        );
    }

    /// Start the (now updated) exe on the same file and tab, then close.
    fn restart(&mut self, ctx: &egui::Context) {
        let exe = match update::exe_path() {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(e);
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
            Err(e) => self.error = Some(format!("Cannot start {}: {e}", exe.display())),
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
                        self.included = l
                            .book
                            .sections
                            .iter()
                            .map(|s| s.default_included())
                            .collect();
                    }
                    self.book = Some(l.book);
                    self.prep = Some(l.prep);
                    self.rebuild_report();
                    self.fit_pending = true;
                    // Anki sentences must be segmented like this book.
                    if self.anki.col.is_some()
                        && self.anki.built_with.as_ref() != Some(&self.anki_seg_config())
                    {
                        self.anki_rebuild(ctx);
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_job(&ctx);
        self.poll_suggestions(&ctx);
        self.poll_anki(&ctx);

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if let Some(p) = dropped.into_iter().next() {
            self.open(p, &ctx);
        }

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("📂 Open…").clicked()
                    && let Some(p) = rfd::FileDialog::new()
                        .add_filter("E-books and text", &["epub", "txt", "text"])
                        .add_filter("All files", &["*"])
                        .pick_file()
                {
                    self.open(p, &ctx);
                }
                ui.separator();
                ui.label("Segmenter:");
                let before = self.segmenter;
                egui::ComboBox::from_id_salt("seg").selected_text(self.segmenter.label()).show_ui(ui, |ui| {
                    for k in SegmenterKind::ALL {
                        ui.selectable_value(&mut self.segmenter, k, k.label()).on_hover_text(k.description());
                    }
                });
                if before != self.segmenter && self.job.is_none() {
                    self.rerun(&ctx, false);
                }

                let dict_label = match (&self.dict_path, self.user_words.len()) {
                    (Some(p), n) => format!("📖 {} ({n} words)", file_name(p)),
                    (None, 0) => "📖 User dictionary…".to_owned(),
                    (None, n) => format!("📖 {n} forced words"),
                };
                if ui
                    .button(dict_label)
                    .on_hover_text("Plain text, one word per line (names, terms). Forces these to be segmented as single words.")
                    .clicked()
                    && let Some(p) = rfd::FileDialog::new().add_filter("Text", &["txt", "dict"]).pick_file()
                {
                    match hanstat::read_user_dict(&p) {
                        Ok(w) => {
                            self.user_words = w;
                            self.dict_path = Some(p);
                            self.rerun(&ctx, false);
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                if !self.user_words.is_empty() && ui.small_button("✖").on_hover_text("Remove all forced words").clicked() {
                    self.dict_path = None;
                    self.user_words.clear();
                    self.rerun(&ctx, false);
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
                        if self.anki.settings.collection.is_none() {
                            self.anki.profiles = anki::find_profiles();
                            self.anki.settings.collection =
                                self.anki.profiles.first().map(|p| p.collection.clone());
                        }
                        self.anki_open(&ctx);
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
                    if before != self.encoding && self.job.is_none() {
                        self.rerun(&ctx, true);
                    }
                }

                if self.report.is_some() {
                    ui.separator();
                    ui.menu_button("💾 Export", |ui| {
                        let stem = self.path.as_deref().map(file_stem).unwrap_or_else(|| "hanstat".into());
                        let r = self.report.as_ref().unwrap();
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
                            && ui.button("Words not young/mature in Anki, by frequency (TXT)…").clicked()
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
                }
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
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
                    ui.label(p.display().to_string());
                } else {
                    ui.label("Open or drop an .epub or .txt file.");
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
                                Color32::from_rgb(220, 170, 60),
                                "⚠ Model download failed; it will be retried when the model is needed",
                            )
                            .on_hover_text(errors.join("\n\n"));
                        }
                    }
                }
                self.update_status(ui, &ctx);
                let now = ui.input(|i| i.time);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.version_menu(ui, &ctx);
                    if let Some((m, until)) = &self.toast {
                        if now < *until {
                            ui.label(RichText::new(m).strong());
                            ctx.request_repaint_after(std::time::Duration::from_millis(250));
                        } else {
                            self.toast = None;
                        }
                    }
                });
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(e) = &self.error {
                ui.colored_label(Color32::from_rgb(220, 80, 80), format!("⚠ {e}"));
                ui.separator();
            }
            if self.report.is_none() {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new(if self.job.is_some() {
                            "Analysing…"
                        } else {
                            "Drop an .epub or .txt file here, or click Open…"
                        })
                        .size(22.0)
                        .weak(),
                    );
                });
                return;
            }
            ui.horizontal(|ui| {
                for (t, name) in [
                    (Tab::Overview, "Overview"),
                    (Tab::Characters, "Characters"),
                    (Tab::Words, "Words"),
                    (Tab::Sections, "Sections"),
                    (Tab::Suggestions, "Suggested words"),
                    (Tab::Notes, "Notes & method"),
                ] {
                    ui.selectable_value(&mut self.tab, t, RichText::new(name).size(15.0));
                }
            });
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
    }
}

/// Grow the window's inner height to `want` points, without going past the
/// bottom of the screen (minus the taskbar). Never shrinks; does nothing when
/// maximised or full-screen.
fn grow_window(ctx: &egui::Context, want: f32) {
    /// Windows' taskbar at 100–200% scaling is 40–48 points tall.
    const TASKBAR: f32 = 48.0;
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
    ui.label("Anki:");
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
    egui::Frame::group(ui.style())
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_min_width(230.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(title).weak());
                ui.label(RichText::new(fmt_int(value)).size(30.0).strong());
                ui.label(sub);
            });
        });
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
            if ui
                .link(RichText::new(msg).color(Color32::from_rgb(220, 170, 60)))
                .clicked()
            {
                self.tab = Tab::Suggestions;
            }
            ui.add_space(4.0);
        }
        let r = self.report.as_ref().unwrap();
        let out = egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if let Some(t) = &r.title {
                ui.heading(t);
            }
            ui.label(format!(
                "{} · {:?} Chinese (traditional-only chars: {}, simplified-only: {}) · {} of {} sections included",
                r.format,
                r.variant,
                fmt_int(r.trad_marks),
                fmt_int(r.simp_marks),
                self.included.iter().filter(|x| **x).count(),
                self.included.len()
            ));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                big_stat(ui, "Chinese characters (total)", r.han_total, &format!("{} occur only once", fmt_int(r.han_hapax as u64)));
                big_stat(ui, "Unique characters", r.han_unique as u64, &format!("{} needed for 99% coverage", r.char_coverage.iter().find(|c| c.percent == 99.0).map(|c| c.items).unwrap_or(0)));
                big_stat(ui, "Chinese words (total)", r.words_total, &format!("segmenter: {}", r.segmenter.label()));
                big_stat(ui, "Unique words", r.words_unique as u64, &format!("{} occur only once", fmt_int(r.words_hapax as u64)));
            });
            ui.add_space(10.0);

            if let Some(c) = &r.anki {
                ui.heading("Known from Anki");
                ui.horizontal_wrapped(|ui| {
                    for (title, k) in [
                        ("Unique characters you know", &c.unique_chars),
                        ("Unique words you know", &c.unique_words),
                    ] {
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
                });
                ui.label(
                    RichText::new(format!(
                        "Known = on a young or mature card in your selected decks ({} words, {} characters). A character is known if it appears in a known word.",
                        fmt_int(c.anki.words.known()),
                        fmt_int(c.anki.chars.known()),
                    ))
                    .weak(),
                );
                ui.add_space(10.0);
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
                        let mins = r.han_total as f64 / self.cpm as f64;
                        ui.label(format!("{}h {:02}m at", (mins / 60.0) as u64, (mins % 60.0) as u64));
                        ui.add(egui::DragValue::new(&mut self.cpm).range(50.0..=2000.0).speed(5.0).suffix(" chars/min"));
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
        if now < self.fit_until && !ctx.will_discard() {
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

    fn char_table(&mut self, ui: &mut egui::Ui) {
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
        let status = r.anki.as_ref().map(|c| &c.char_status);
        let af = self.anki_filter;
        let f: Vec<char> = self
            .char_filter
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let rows: Vec<usize> = (0..r.char_freq.len())
            .filter(|&i| f.is_empty() || f.contains(&r.char_freq[i].0))
            .filter(|&i| status.is_none_or(|s| af.matches(s[i])))
            .collect();
        let total = r.han_total.max(1) as f64;
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::exact(70.0))
            .column(Column::exact(70.0))
            .column(Column::exact(100.0))
            .column(Column::exact(90.0))
            .column(Column::exact(110.0))
            .column(Column::exact(if status.is_some() { 100.0 } else { 0.0 }))
            .column(Column::remainder())
            .header(24.0, |mut h| {
                let anki_h = if status.is_some() { "Anki" } else { "" };
                for t in [
                    "Rank",
                    "Char",
                    "Count",
                    "%",
                    "Cumulative %",
                    anki_h,
                    "Code point",
                ] {
                    h.col(|ui| {
                        ui.strong(t);
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
    }

    fn word_table(&mut self, ui: &mut egui::Ui) {
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
        });
        let status = r.anki.as_ref().map(|c| &c.word_status);
        let af = self.anki_filter;
        let f = self.word_filter.trim();
        let rows: Vec<usize> = (0..r.word_freq.len())
            .filter(|&i| f.is_empty() || r.word_freq[i].0.contains(f))
            .filter(|&i| status.is_none_or(|s| af.matches(s[i])))
            .collect();
        let total = r.words_total.max(1) as f64;
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::exact(70.0))
            .column(Column::exact(200.0))
            .column(Column::exact(100.0))
            .column(Column::exact(90.0))
            .column(Column::exact(110.0))
            .column(Column::exact(if status.is_some() { 100.0 } else { 0.0 }))
            .column(Column::remainder())
            .header(24.0, |mut h| {
                let anki_h = if status.is_some() { "Anki" } else { "" };
                for t in [
                    "Rank",
                    "Word",
                    "Count",
                    "%",
                    "Cumulative %",
                    anki_h,
                    "Length",
                ] {
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|body| {
                body.rows(26.0, rows.len(), |mut row| {
                    let i = rows[row.index()];
                    let (w, n) = &r.word_freq[i];
                    row.col(|ui| {
                        ui.label((i + 1).to_string());
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
                });
            });
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
                for t in [
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
                ] {
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
                    for v in [
                        fmt_int(s.han),
                        fmt_int(s.unique_han as u64),
                        dash(s.new_han),
                        fmt_int(s.words),
                        fmt_int(s.unique_words as u64),
                        dash(s.new_words),
                    ] {
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
        let Some(cands) = &self.suggestions else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Looking for book-specific words…");
            });
            return;
        };
        ui.label(
            "Strings that recur in this book, behave like single words (their parts rarely occur apart; many different \
             characters appear on either side), but are split by the segmenter. These are mostly character names, places \
             and in-world terms. Pre-ticked = high confidence (unknown to a general dictionary, and either the segmenter already \
             keeps it whole part of the time or the statistical evidence is very strong). Applying forces the ticked \
             strings to always count as one word; save them via Export to reuse for the rest of a series.",
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
        });
        let mut toggles: Vec<String> = Vec::new();
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::exact(28.0))
            .column(Column::exact(150.0))
            .column(Column::exact(100.0))
            .column(Column::exact(110.0))
            .column(Column::exact(100.0))
            .column(Column::exact(110.0))
            .column(Column::remainder())
            .header(24.0, |mut h| {
                for (t, tip) in [
                    ("", ""),
                    ("Word", ""),
                    ("Occurrences", "Times the string occurs in the included sections"),
                    ("Split", "Occurrences the segmenter did NOT keep as one word"),
                    ("Dependency", "Share of the rarer part's occurrences that are inside this string (1.0 = parts never occur apart)"),
                    ("Boundary freedom", "Min of left/right neighbour entropy (nats); high = appears in many contexts"),
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
                    row.col(|ui| {
                        ui.label(RichText::new(&c.word).size(18.0));
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
                            ui.label(RichText::new("recommended").color(Color32::from_rgb(90, 170, 110)));
                        } else if c.in_dictionary {
                            ui.label(RichText::new("general word").weak())
                                .on_hover_text("In jieba's general dictionary: the split is usually a granularity choice, not an error.");
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
        if apply {
            self.apply_selected(ctx);
        }
    }

    fn notes(&mut self, ui: &mut egui::Ui) {
        let r = self.report.as_ref().unwrap();
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
                ui.add_space(12.0);
                ui.heading("Models");
                for m in [&hanstat::models::LTP, &hanstat::models::CKIP] {
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
                ui.heading("How things are counted");
                for line in METHOD {
                    ui.label(format!("• {line}"));
                }
            });
    }
}

const METHOD: &[&str] = &[
    "Chinese character = a code point with Unicode Script=Han that is a letter (all CJK Unified Ideograph blocks incl. Extensions A–J and supplementary planes, compatibility ideographs, 〇, 々). Punctuation, radicals, Latin, digits, whitespace are never counted as Chinese characters.",
    "Text is NFC-normalised first: compatibility ideographs become their unified form, Kangxi radicals used as characters are folded, variation selectors are dropped (葛󠄀 and 葛 are one character), invisible characters (zero-width space, soft hyphen, BOM) are removed.",
    "EPUB: only documents in the spine are read, in reading order, each once. <head>, scripts, styles, ruby annotations (<rt>/<rp>), hidden elements and image alt text are excluded. Block elements and <br> end paragraphs; inline tags never split words. Line breaks in the HTML source between Chinese characters are removed, as browsers do.",
    "TXT: encoding from BOM, strict UTF-8 validation, UTF-16 statistics, then chardetng (GBK is read as GB18030). Each line is a paragraph unless the file is detected as hard-wrapped.",
    "Words: there is no single correct segmentation of Chinese; different standards (PKU, CTB, MSR, AS) disagree on 5–15% of boundaries, and unique-word counts can differ by 20–40% between segmenters. Only tokens containing Chinese characters are counted as Chinese words. Character counts, by contrast, are exact.",
    "Segmenters (SIGHAN 2005 F1): LTP perceptron is best on Simplified text (MSR 87.7, PKU 95.2) and takes seconds; CKIP BERT is best on Traditional text (CityU 90.1, AS 97.9) and keeps character names together more consistently, but takes minutes per novel; jieba is fastest and least accurate (81–84). Auto picks LTP for Simplified and CKIP for Traditional.",
    "Proper names are the main source of segmentation errors. Load a user dictionary (one word per line) to fix them; it applies to every segmenter.",
    "Script folding for the segmenter only: Traditional text is folded to Simplified for LTP/jieba (character-level), and Simplified text to Traditional for CKIP (OpenCC phrase-level, e.g. 以后→以後), matching each model's training script. The conversion is length-preserving and all counts and lists keep the original characters.",
    "Sentences end at 。！？!?… (runs count once) or the end of a paragraph; a sentence must contain a letter or digit.",
    "Coverage = the number of most-frequent distinct characters/words whose occurrences add up to X% of all occurrences.",
    "Anki: the collection is copied (with its write-ahead log) to a temporary folder and read from there, so it works while Anki is open and is never modified. A review card is mature at an interval of 21+ days and young below that; learning and relearning cards are 'learning'. A note takes its best card's status in the selected decks, a word its best note's, and a character the best status of any known word containing it. A ticked field value with sentence punctuation is split into words with the book's segmenter and forced words, so its boundaries agree with the book's. Otherwise each run of Chinese characters is a word; runs longer than 4 characters (idioms, sayings) also count their segmented parts. Book words must match exactly (after optional Traditional→Simplified folding), so the segmenter's word boundaries matter.",
];
